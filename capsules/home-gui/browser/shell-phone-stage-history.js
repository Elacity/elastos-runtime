/* One-entry history model for the phone stage; pure so node:test can drive
   it. The shell pushes one entry when its first layer opens and consumes it
   when the last layer closes, so back on the bare desktop leaves Home.

   history.state is the source of truth, not a flag: capsule frames add joint
   session entries of their own above ours and traversal is asynchronous, so
   counting pushes and pops drifts (WebKit delivered pops to the wrong step).
   Reading which entry we landed on cannot drift.

   Leaving calls back() once per close cycle and never retries: a second
   back() before the traversal lands could walk out of the host's own
   history. If that one back() only popped a capsule step, our entry stays
   until the next open/close cycle or the user's next back consumes it — one
   extra back press, never a navigation away. */

export const STAGE_HISTORY_STATE = Object.freeze({ elastosStage: true });

export function isStageHistoryState(state) {
  return Boolean(state && typeof state === "object" && state.elastosStage === true);
}

/**
 * @param {object} io
 * @param {() => boolean} io.onOurEntry — is history.state ours right now
 * @param {() => number} io.layerCount — windows + sheets + Mission Control
 * @param {() => void} io.pushState — push STAGE_HISTORY_STATE
 * @param {() => void} io.back — history.back()
 * @param {() => number} io.onBack — close the top layer; returns layers left
 */
export function createStageHistory({ onOurEntry, layerCount, pushState, back, onBack }) {
  let leaving = false;

  const leave = () => {
    if (leaving) {
      return;
    }
    leaving = true;
    back();
  };

  return {
    get leaving() {
      return leaving;
    },
    // Called whenever the layer count may have changed.
    sync() {
      const layers = layerCount();
      const ours = onOurEntry();
      if (layers > 0) {
        // A new cycle: whatever the last leave did has long landed.
        leaving = false;
        if (!ours) {
          pushState();
        }
        return;
      }
      if (ours) {
        leave();
      }
    },
    // A history traversal landed on this document. Returns whether the shell
    // handled it.
    handlePopState() {
      leaving = false;
      const layers = layerCount();
      const ours = onOurEntry();
      if (layers === 0) {
        // Our own leave completed (not ours), or a leftover entry of ours
        // surfaced with nothing open: the user is on the way out.
        if (ours) {
          leave();
        }
        return false;
      }
      // Something is open and the user went back: from above our entry (a
      // capsule step was popped) or off it (we sit beneath it now).
      const remaining = onBack();
      if (remaining > 0 && !ours) {
        pushState();
      } else if (remaining === 0 && ours) {
        leave();
      }
      return true;
    },
  };
}
