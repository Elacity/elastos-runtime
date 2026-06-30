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

export const MAC_VM_MANUAL_CHECKS = [
  "mac_gateway_restarted",
  "remote_video_visible",
  "typing_latency",
  "address_bar_stability",
  "scrolling_click_fidelity",
  "performance_thresholds_reviewed",
  "zoom_geometry_reviewed",
  "ela_city_url_sync",
  "ela_city_images_loaded",
  "ela_city_edit_profile_modal",
  "no_raw_authority",
  "session_cleanup",
];

export function requiredManualChecksForSchema(schema) {
  if (schema === "elastos.browser.mac-vm-proof/v1") {
    return [...MAC_VM_MANUAL_CHECKS];
  }
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
  return [...new Set([...COMMON_MANUAL_CHECKS, ...HOSTED_WEBRTC_MANUAL_CHECKS, ...MAC_VM_MANUAL_CHECKS])];
}
