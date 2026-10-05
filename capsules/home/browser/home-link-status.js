/* Home link status. The host page is the one document that talks to this
   Home's gateway. When the gateway stops answering (a phone leaving Wi-Fi,
   the gateway restarting) the host tells the active shell
   (`home:link-status`), so the shell says "Reconnecting" instead of quietly
   showing stale state. While the link is down the host retries on a short
   timer; the first answer clears it. Presentation only — no authority.
   See docs/HOME_MOBILE.md. */

export const LINK_STATUS_MESSAGE = "home:link-status";
export const LINK_RETRY_MS = 3_000;
// A proxy in front of the gateway answering for it while it is gone.
const GATEWAY_GONE_STATUSES = new Set([502, 503, 504]);

// fetch rejects with a TypeError in every engine when the request never
// reaches the gateway; an HTTP answer carries `status` (shell-core fetchJson).
// A request that gets no answer before its deadline rejects with the
// deadline's TimeoutError; a cancelled request (AbortError) is not an outage.
export function isGatewayUnreachable(error) {
  if (error instanceof TypeError || error?.name === "TimeoutError") {
    return true;
  }
  return GATEWAY_GONE_STATUSES.has(Number(error?.status));
}

// `post` delivers to the active shell; `retry` asks the gateway again (the
// host's summary refresh, whose answer is what the shell needs anyway).
export function createHomeLinkStatus({
  post,
  retry,
  retryMs = LINK_RETRY_MS,
  setTimer = (callback, ms) => window.setTimeout(callback, ms),
  clearTimer = (timer) => window.clearTimeout(timer),
}) {
  let reachable = true;
  let retryTimer = null;

  const announce = () => post({ type: LINK_STATUS_MESSAGE, reachable });
  const cancelRetry = () => {
    if (retryTimer !== null) {
      clearTimer(retryTimer);
      retryTimer = null;
    }
  };
  const markReachable = () => {
    cancelRetry();
    if (!reachable) {
      reachable = true;
      announce();
    }
  };

  return {
    reachable: () => reachable,
    // True when the failure means the gateway is unreachable (the caller has
    // nothing more to report); false for any answer the gateway gave.
    reportFailure(error) {
      if (!isGatewayUnreachable(error)) {
        // Any other HTTP status is an answer, so the link is back.
        if (Number.isInteger(error?.status)) {
          markReachable();
        }
        return false;
      }
      if (reachable) {
        reachable = false;
        announce();
      }
      cancelRetry();
      retryTimer = setTimer(() => {
        retryTimer = null;
        retry();
      }, retryMs);
      return true;
    },
    reportSuccess: markReachable,
    // A reloaded shell starts out reachable; tell it again while down.
    replay() {
      if (!reachable) {
        announce();
      }
    },
  };
}
