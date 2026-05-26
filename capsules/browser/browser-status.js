export function requestedDisplayMode(paramsArg = params, debugMetricsArg = debugMetrics) {
  const value = paramsArg.get("display_mode") || paramsArg.get("display") || "webrtc_remote_display";
  if (value === "diagnostic" || value === "diagnostic_frame") {
    if (!debugMetricsArg) {
      throw new Error("Diagnostic Browser display mode requires debug=1 or metrics=1.");
    }
    return "diagnostic_frame";
  }
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

export function friendlyOpenError(error) {
  const text = error.message || "Browser request failed";
  if (isAuthoritySessionError(error)) {
    return "Browser authority expired. Relaunching through Home...";
  }
  if (error.status === 403) {
    return `Blocked by Browser Exit policy: ${text}`;
  }
  if (error.status === 503) {
    return `Browser failed closed: ${text}`;
  }
  return text;
}

export async function collectWebrtcStats(peerConnection) {
  if (!peerConnection || typeof peerConnection.getStats !== "function") {
    return null;
  }
  const report = await peerConnection.getStats();
  const stats = {};
  for (const item of report.values()) {
    const mediaKind = item.kind || item.mediaType;
    const isInboundVideo =
      item.type === "inbound-rtp" &&
      (mediaKind === "video" || "framesDecoded" in item || "framesPerSecond" in item);
    const isInboundAudio =
      item.type === "inbound-rtp" &&
      !isInboundVideo &&
      (mediaKind === "audio" || "audioLevel" in item || "totalAudioEnergy" in item);
    if (isInboundVideo) {
      stats.video_frames_decoded = Number(item.framesDecoded || 0);
      stats.video_frames_dropped = Number(item.framesDropped || 0);
      stats.video_fps = Number(item.framesPerSecond || 0);
      stats.video_bytes_received = Number(item.bytesReceived || 0);
      stats.video_packets_lost = Number(item.packetsLost || 0);
      stats.video_jitter_ms = Number(item.jitter || 0) * 1000;
    } else if (isInboundAudio) {
      stats.audio_bytes_received = Number(item.bytesReceived || 0);
      stats.audio_packets_lost = Number(item.packetsLost || 0);
      stats.audio_jitter_ms = Number(item.jitter || 0) * 1000;
    } else if (item.type === "candidate-pair" && item.state === "succeeded" && item.nominated) {
      stats.rtt_ms = Number(item.currentRoundTripTime || 0) * 1000;
      stats.available_incoming_bitrate = Number(item.availableIncomingBitrate || 0);
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
