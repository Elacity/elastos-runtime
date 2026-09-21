/* Assistant mark: the Elastos chevrons on the dock toggle.

   Two layers in one square — a frosted-glass upper chevron over the complete
   orange lower chevron — composited live, so the orange seen through the glass
   is real alpha, not baked into the glass artwork. Still at rest: no load,
   hover or idle motion.

   A real activation (pointer or keyboard click) plays the whole contact
   before anything else moves: the chevrons close toward each other
   (CONTACT_MS), then return to rest (RETURN_MS), and only then does the face
   open. Equal and opposite travel keeps both chevrons exactly aligned at
   contact. The return ends on the resting transform, so cancelling the
   filled animations afterwards changes nothing on screen.

   Presentation only — no authority. Repeat clicks are ignored until the
   sequence settles; reduced motion opens immediately. */

import { prefersReducedMotion } from "./shell-motion.js?v=home-20260813a";

export const ASSISTANT_MARK_CONTACT_MS = 240;
export const ASSISTANT_MARK_RETURN_MS = 170;
/* Share of the square the glass travels down; the orange travels the same
   share up, so the chevrons meet exactly. */
const CONTACT_TRAVEL = "10.35%";
const REST = "0";
const EASE_CLOSE = "cubic-bezier(0.32, 0, 0.18, 1)";
const EASE_RETURN = "cubic-bezier(0.2, 0, 0, 1)";

function layerEl(root, layer) {
  return root.querySelector(`[data-assistant-layer="${layer}"]`);
}

function opposite(travel) {
  return travel === REST ? REST : `-${travel}`;
}

function translateFrames(from, to) {
  return [
    { transform: `translate3d(0, ${from}, 0)` },
    { transform: `translate3d(0, ${to}, 0)` },
  ];
}

/* Cancelled animations reject — that only means the mark was torn down. */
async function settled(animations) {
  try {
    await Promise.all(animations.map((animation) => animation.finished));
  } catch {
    /* torn down mid-flight */
  }
}

/* Both chevrons travel together, the orange mirroring the glass. */
function moveChevrons(art, from, to, duration, easing) {
  const options = { duration, easing, fill: "forwards" };
  return [
    layerEl(art, "glass").animate(translateFrames(from, to), options),
    layerEl(art, "orange").animate(translateFrames(opposite(from), opposite(to)), options),
  ];
}

/* Close, return to rest, then release the filled transforms. */
async function playContact(art) {
  const close = moveChevrons(art, REST, CONTACT_TRAVEL, ASSISTANT_MARK_CONTACT_MS, EASE_CLOSE);
  await settled(close);
  const back = moveChevrons(art, CONTACT_TRAVEL, REST, ASSISTANT_MARK_RETURN_MS, EASE_RETURN);
  await settled(back);
  [...close, ...back].forEach((animation) => animation.cancel());
}

/**
 * Wire the mark on the Assistant toggle.
 * @param {HTMLElement | null} button
 * @param {{
 *   onActivate: () => void,
 *   animate?: () => boolean,
 *   reducedMotion?: () => boolean,
 * }} options `animate` lets the caller skip the contact when the activation
 *   is not an opening (closing an open face needs no chevron motion).
 * @returns {() => void} unbind
 */
export function bindAssistantMark(button, options) {
  if (!button) {
    return () => {};
  }
  const shouldAnimate = options.animate ?? (() => true);
  const reducedMotion = options.reducedMotion ?? prefersReducedMotion;
  let running = false;

  async function handleClick(event) {
    event.preventDefault();
    if (running) {
      return;
    }
    const art = button.querySelector(".assistant-mark");
    if (!art || reducedMotion() || !shouldAnimate()) {
      options.onActivate();
      return;
    }
    running = true;
    button.dataset.animating = "true";
    await playContact(art);
    delete button.dataset.animating;
    running = false;
    options.onActivate();
  }

  button.addEventListener("click", handleClick);
  return () => button.removeEventListener("click", handleClick);
}
