export function requestedDisplayMode(paramsArg = params, debugMetricsArg = debugMetrics) {
  const value = paramsArg.get("display_mode") || paramsArg.get("display") || "webrtc_remote_display";
  if (["webrtc_remote_display", "native_surface"].includes(value)) {
    return value;
  }
  throw new Error("Unsupported Browser display mode.");
}

export function isMissingRuntimePageError(error) {
  const text = String(error?.message || "");
  return error?.status === 404 || /browser page not found|page not found/i.test(text);
}

export function isAuthoritySessionError(error) {
  const text = String(error?.message || "");
  return (
    error?.status === 401 ||
    error?.status === 403 ||
    /auth session not found|auth session is not active|home launch token auth session is not active|home launch token expired/i.test(text)
  );
}

function sanitizedErrorText(error) {
  const text = String(error?.message || "Browser request failed");
  return text
    .replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "")
    .replace(/[\r\n\t]+/g, " ")
    .replace(/\s{2,}/g, " ")
    .trim()
    .slice(0, 420);
}

export function runtimeOpenOutcome(error) {
  const outcome = error?.payload?.outcome;
  const effects = outcome?.effects;
  const indeterminateLaunch =
    outcome?.state === "cleanup_pending" &&
    outcome?.ownership === "launch_reconciliation_pending" &&
    effects?.page_acquired === null &&
    effects?.vm_acquired === null &&
    effects?.stream_acquired === true;
  if (
    outcome?.schema !== "elastos.browser.open-outcome/v1" ||
    ![
      "terminal_pre_effect_failure",
      "terminal_post_effect_cleanup",
      "cleanup_pending",
    ].includes(outcome.state) ||
    !effects ||
    (!indeterminateLaunch &&
      (typeof effects.page_acquired !== "boolean" ||
        typeof effects.vm_acquired !== "boolean" ||
        typeof effects.stream_acquired !== "boolean"))
  ) {
    return null;
  }
  return outcome;
}

export function friendlyOpenError(error) {
  const text = sanitizedErrorText(error);
  if (isAuthoritySessionError(error)) {
    return "Browser session expired. Reopening from Home...";
  }
  const outcome = runtimeOpenOutcome(error);
  if (outcome?.state === "terminal_pre_effect_failure") {
    return "Browser Engine failed to start cleanly. No Browser page or VM was acquired.";
  }
  if (outcome?.state === "terminal_post_effect_cleanup") {
    return "Browser Engine failed to start cleanly. Runtime confirmed the acquired Browser effects were closed.";
  }
  if (outcome?.state === "cleanup_pending") {
    if (outcome.ownership === "launch_reconciliation_pending") {
      return "Browser Engine returned no safe launch result. Runtime retained ownership and is reconciling before another Browser session can start.";
    }
    if (
      outcome.effects.page_acquired === true ||
      outcome.effects.vm_acquired === true
    ) {
      return "Browser Engine failed to start cleanly. Runtime cleanup is pending for the acquired Browser session.";
    }
    return "Browser Engine failed to start cleanly. Runtime is finishing cleanup; no Browser page or VM remains acquired.";
  }
  if (error.status === 403) {
    return "This page was blocked by your Exit Node settings.";
  }
  if (error.status === 503) {
    return "Browser is temporarily unavailable. Refresh Browser or choose another Browser Engine.";
  }
  if (Number(error?.status) >= 400) {
    return "Browser could not complete the request. Refresh Browser and try again.";
  }
  if (/\b(schema|projection|provider|adapter|capability|affordance|runtime-owned|launch token|hostcall|request failed|failed to fetch|unauthorized|forbidden|[45]\d\d)\b|engine_[a-z_]+/i.test(text)) {
    return "Browser could not complete the request. Refresh Browser and try again.";
  }
  return text;
}

export async function collectWebrtcStats(peerConnection) {
  if (!peerConnection || typeof peerConnection.getStats !== "function") {
    return null;
  }
  const report = await peerConnection.getStats();
  const reportItems = [...report.values()];
  const reportById = new Map(
    reportItems
      .filter((item) => typeof item?.id === "string")
      .map((item) => [item.id, item]),
  );
  const stats = {};
  for (const item of reportItems) {
    const mediaKind = item.kind || item.mediaType;
    const isInboundVideo =
      item.type === "inbound-rtp" &&
      (mediaKind === "video" || "framesDecoded" in item || "framesPerSecond" in item);
    const isInboundAudio =
      item.type === "inbound-rtp" &&
      !isInboundVideo &&
      (
        mediaKind === "audio" ||
        "audioLevel" in item ||
        "totalAudioEnergy" in item ||
        "bytesReceived" in item
      );
    if (isInboundVideo) {
      stats.video_frames_decoded = Number(item.framesDecoded || 0);
      stats.video_frames_dropped = Number(item.framesDropped || 0);
      stats.video_fps = Number(item.framesPerSecond || 0);
      stats.video_bytes_received = Number(item.bytesReceived || 0);
      stats.video_packets_received = Number(item.packetsReceived || 0);
      stats.video_packets_lost = Number(item.packetsLost || 0);
      stats.video_jitter_ms = Number(item.jitter || 0) * 1000;
    } else if (isInboundAudio) {
      stats.audio_bytes_received = Number(item.bytesReceived || 0);
      stats.audio_packets_received = Number(item.packetsReceived || 0);
      stats.audio_packets_lost = Number(item.packetsLost || 0);
      stats.audio_jitter_ms = Number(item.jitter || 0) * 1000;
    } else if (item.type === "candidate-pair" && item.state === "succeeded" && item.nominated) {
      const localCandidate = reportById.get(item.localCandidateId);
      const remoteCandidate = reportById.get(item.remoteCandidateId);
      stats.rtt_ms = Number(item.currentRoundTripTime || 0) * 1000;
      stats.available_incoming_bitrate = Number(item.availableIncomingBitrate || 0);
      stats.selected_local_candidate_type =
        String(localCandidate?.candidateType || "unknown");
      stats.selected_remote_candidate_type =
        String(remoteCandidate?.candidateType || "unknown");
      stats.selected_protocol = String(
        localCandidate?.protocol || remoteCandidate?.protocol || "unknown",
      );
    }
  }
  return stats;
}

export function browserMetricsText(status, {
  latestWebrtcStats,
  remoteAudioExpected,
  remoteAudioUnlocked,
  remoteVideo,
}) {
  const frameAge =
    Number.isFinite(Number(status.last_frame_age_ms))
      ? `${Math.round(Number(status.last_frame_age_ms))}ms`
      : "n/a";
  const decode =
    Number.isFinite(Number(status.last_frame_decode_ms))
      ? `${Math.round(Number(status.last_frame_decode_ms))}ms`
      : "n/a";
  const size =
    Number(status.last_frame_width) && Number(status.last_frame_height)
      ? `${status.last_frame_width}x${status.last_frame_height}`
      : "n/a";
  const videoFps =
    Number.isFinite(Number(latestWebrtcStats?.video_fps))
      ? `${Math.round(Number(latestWebrtcStats.video_fps))}fps`
      : "n/a";
  const videoBytes =
    Number.isFinite(Number(latestWebrtcStats?.video_bytes_received))
      ? `${Math.round(Number(latestWebrtcStats.video_bytes_received) / 1024)}KiB`
      : "n/a";
  const audioBytes =
    Number.isFinite(Number(latestWebrtcStats?.audio_bytes_received))
      ? `${Math.round(Number(latestWebrtcStats.audio_bytes_received) / 1024)}KiB`
      : "n/a";
  const audioState = remoteAudioExpected ? (remoteAudioUnlocked ? "on" : "muted") : "n/a";
  const rtt =
    Number.isFinite(Number(latestWebrtcStats?.rtt_ms))
      ? `${Math.round(Number(latestWebrtcStats.rtt_ms))}ms`
      : "n/a";
  const decodedFrames = Number(remoteVideo.webkitDecodedFrameCount || 0);
  const droppedFrames = Number(remoteVideo.webkitDroppedFrameCount || 0);
  return [
    `backend ${status.display_backend || "n/a"}`,
    `frames ${Number(status.frame_count || 0)}`,
    `drop ${Number(status.dropped_frames || 0)}`,
    `age ${frameAge}`,
    `decode ${decode}`,
    `size ${size}`,
    `ice ${status.ice_connection_state || "n/a"}`,
    `rtc ${status.webrtc_connection_state || "n/a"}`,
    `fps ${videoFps}`,
    `rx ${videoBytes}`,
    `audio ${audioState}`,
    `arx ${audioBytes}`,
    `rtt ${rtt}`,
    `decoded ${decodedFrames}`,
    `vdrop ${droppedFrames}`,
  ].join(" | ");
}
