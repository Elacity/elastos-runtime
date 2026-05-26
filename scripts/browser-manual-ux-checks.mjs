export const COMMON_MANUAL_CHECKS = [
  "typing_latency",
  "address_bar_stability",
  "scrolling_click_fidelity",
  "youtube_audible_audio",
  "glide_wallet_connect",
  "no_raw_authority",
  "session_cleanup",
];

export const HOSTED_WEBRTC_MANUAL_CHECKS = [
  "display_session_audio_advertised",
  "audio_unlock_gesture",
  "remote_audio_unmuted_status",
  "received_audio_evidence",
];

export function requiredManualChecksForSchema(schema) {
  const checks = [...COMMON_MANUAL_CHECKS];
  if (schema === "elastos.browser.hosted-provider-bakeoff/v1") {
    checks.push(...HOSTED_WEBRTC_MANUAL_CHECKS);
  }
  return checks;
}

export function templateManualChecksForSchema(schema) {
  if (schema) {
    return requiredManualChecksForSchema(schema);
  }
  return [...COMMON_MANUAL_CHECKS, ...HOSTED_WEBRTC_MANUAL_CHECKS];
}
