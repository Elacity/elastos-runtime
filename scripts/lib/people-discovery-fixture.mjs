// Shared People discovery summary fixture for the People smokes.
export function discoverySummary({
  configured = false,
  enabled = false,
  status = configured ? (enabled ? "visible" : "off") : "unconfigured",
  statusMessage = configured
    ? (enabled
      ? "Public is on."
      : "Public is off.")
    : "Public isn’t available on this Home.",
  discoveredPeers = [],
  pendingRequestCount = 0,
  remainingSeconds = null,
  remoteVisibilityMayRemainUntil = null,
  remoteVisibilityRemainingSeconds = null,
} = {}) {
  return {
    schema: "elastos.people.discovery/v1",
    configured,
    enabled,
    status,
    status_message: statusMessage,
    expires_at: remainingSeconds === null ? null : 1_000 + remainingSeconds,
    remaining_seconds: remainingSeconds,
    remote_visibility_may_remain_until: remoteVisibilityMayRemainUntil,
    remote_visibility_remaining_seconds: remoteVisibilityRemainingSeconds,
    discovered_count: discoveredPeers.length,
    discovered_peers: discoveredPeers,
    request_count: pendingRequestCount,
  };
}
