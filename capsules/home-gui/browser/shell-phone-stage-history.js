/* One-entry history model for the phone stage; pure so node:test can drive
   it. The shell pushes one entry when a layer opens and none of its entries is
   current, so the system Back lands on this document as a popstate and closes
   exactly one layer.

   The shell never calls back() while anything is open, so no traversal of its
   own is ever in flight and every popstate is the user's Back: one Back at a
   time, nothing to tell apart. history.state is the source of truth, not a
   counter: it cannot drift.

   Closing the last layer by hand (Close, Escape) leaves our entry in place
   rather than racing a back() against the user. The user's next Back pops it
   with nothing open; that Back meant "leave Home", so the shell finishes it
   with one back() of its own. Capsule frames add no entries of their own on a
   phone stage (Library holds its history there), so a Back never steps inside
   an app frame first. */

export const STAGE_HISTORY_STATE = Object.freeze({ elastosStage: true });

export function isStageHistoryState(state) {
  return Boolean(state && typeof state === "object" && state.elastosStage === true);
}

/**
 * @param {object} io
 * @param {() => boolean} io.onOurEntry — is history.state ours right now
 * @param {() => number} io.layerCount — windows + open Back layers (0 off a phone)
 * @param {() => void} io.pushState — push STAGE_HISTORY_STATE
 * @param {() => void} io.back — history.back()
 * @param {() => number} io.onBack — close the top layer; returns layers left
 */
export function createStageHistory({ onOurEntry, layerCount, pushState, back, onBack }) {
  // Whether our entry was current when we last looked: a pop that leaves it
  // with nothing open is the user's Back off a leftover entry.
  let wasOurs = onOurEntry();

  const arm = () => {
    if (layerCount() > 0 && !onOurEntry()) {
      pushState();
    }
    wasOurs = onOurEntry();
  };

  return {
    // Called whenever the layer count may have changed.
    sync: arm,
    // A history traversal landed on this document. Returns whether the shell
    // handled it as Back on an open layer.
    handlePopState() {
      const ours = onOurEntry();
      if (layerCount() === 0) {
        const leftover = wasOurs && !ours;
        wasOurs = ours;
        if (leftover) {
          back();
        }
        return false;
      }
      onBack();
      arm();
      return true;
    },
  };
}
