/* Home link status, shell side. The host says when this Home's gateway stops
   answering (`home:link-status`, home/browser/home-link-status.js). The bar
   shows a quiet "Reconnecting…" and Notification Centre explains it, both
   keyed on body[data-home-link]. Presentation only. docs/HOME_MOBILE.md. */

export const HOME_LINK_RECONNECTING = "reconnecting";
const BAR_STATUS_TEXT = "Reconnecting…";

// Anything but a boolean `reachable` is ignored.
export function applyHomeLinkStatus(message, doc = document) {
  if (typeof message?.reachable !== "boolean") {
    return;
  }
  if (message.reachable) {
    delete doc.body.dataset.homeLink;
  } else {
    doc.body.dataset.homeLink = HOME_LINK_RECONNECTING;
  }
  const barStatus = doc.querySelector("#toolbar-link-status-text");
  if (barStatus) {
    barStatus.textContent = message.reachable ? "" : BAR_STATUS_TEXT;
  }
}
