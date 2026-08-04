export function projectHomeGuiAuthority(body, summary) {
  body.dataset.homeAuthority =
    summary?.authority?.signed_in === true ? "signed" : "unsigned";
}

export function isTrustedHomeGuiMessage(event, expectedSource, expectedOrigin) {
  return event.source === expectedSource && event.origin === expectedOrigin;
}
