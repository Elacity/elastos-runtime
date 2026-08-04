import { collectWebrtcStats } from "./browser-status.js?v=browser-20260730b";
import {
  iceCandidateType,
  normalizeDisplayIceServers,
  normalizeEngineCandidate,
  normalizeIceCandidateForRuntime,
  sdpHasOnlyRelayCandidates,
  stripTrickleCandidatesFromSdp,
  validateRuntimeLaunchTurn,
} from "./browser-webrtc.js?v=browser-20260730a";

const WEBRTC_CONNECT_TIMEOUT_MS = 30000;
const WEBRTC_DISCONNECT_GRACE_MS = 10000;
const WEBRTC_ENGINE_CANDIDATE_POLL_MS = 300;
const WEBRTC_ENGINE_CANDIDATE_POLL_ATTEMPTS = 40;
const WEBRTC_FRAME_WATCH_MS = 3000;

export function createBrowserRemoteDisplay({
  debugMetrics,
  fetchJson,
  friendlyOpenError,
  getCurrentDisplayMode,
  getLastPageStatus,
  handleRemoteInputChannelMessage,
  handleRemoteInputChannelTeardown = () => {},
  onRecoveryRequired,
  remoteVideo,
  renderEmpty,
  renderPanel,
  resetPageStatus,
  setActiveBrowserPage,
  setDisplayInput,
  showStatus,
  updateMetrics,
}) {
  let peerConnection = null;
  let audioPeerConnection = null;
  let inputChannel = null;
  let mediaStream = null;
  let trackReady = false;
  let connectTimer = 0;
  let candidatePollTimer = 0;
  let audioCandidatePollTimer = 0;
  let disconnectTimer = 0;
  let frameWatchTimer = 0;
  let statsTimer = 0;
  let failureStarted = false;
  let lastVideoProgressAt = 0;
  let lastVideoDecodedFrames = 0;
  let lastVideoCurrentTime = 0;
  let latestWebrtcStats = null;
  let latestVideoWebrtcStats = null;
  let latestAudioWebrtcStats = null;
  let remoteAudioExpected = false;
  let remoteAudioUnlocked = false;
  let browserCandidateCount = 0;
  let engineCandidateCount = 0;
  let audioBrowserCandidateCount = 0;
  let audioEngineCandidateCount = 0;
  let audioRawCandidateEventCount = 0;
  let audioNullCandidateEventCount = 0;
  let lastBrowserCandidateSummary = "none";
  let lastAudioBrowserCandidateSummary = "none";
  let lastAudioEngineCandidateSummary = "none";
  let audioOfferSummary = null;
  let audioAnswerSummary = null;
  let engineRelayOnly = false;
  let mediaDiagnosticBinding = null;
  let mediaDiagnosticOrdinal = 0;
  const remoteAudio = new Audio();
  remoteAudio.autoplay = true;
  remoteAudio.muted = true;
  remoteAudio.defaultMuted = true;

  function mediaDiagnosticValue(value, defaultValue = "unknown") {
    const text = String(value || "");
    return /^[a-z][a-z0-9_-]{0,31}$/.test(text) ? text : defaultValue;
  }

  function mediaDiagnosticNumber(value) {
    const number = Number(value || 0);
    return Number.isSafeInteger(number) && number >= 0 ? number : 0;
  }

  function mediaDiagnosticStats(stats) {
    return {
      video_packets_received: mediaDiagnosticNumber(
        stats?.video_packets_received,
      ),
      video_bytes_received: mediaDiagnosticNumber(stats?.video_bytes_received),
      audio_packets_received: mediaDiagnosticNumber(
        stats?.audio_packets_received,
      ),
      audio_bytes_received: mediaDiagnosticNumber(stats?.audio_bytes_received),
      selected_local_candidate_type: mediaDiagnosticValue(
        stats?.selected_local_candidate_type,
      ),
      selected_remote_candidate_type: mediaDiagnosticValue(
        stats?.selected_remote_candidate_type,
      ),
      selected_protocol: mediaDiagnosticValue(stats?.selected_protocol),
    };
  }

  function emitMediaDiagnostic(event, channel, fields = {}) {
    if (!mediaDiagnosticBinding || mediaDiagnosticOrdinal >= 128) {
      return;
    }
    console.info(JSON.stringify({
      ...fields,
      schema: "elastos.browser.media-diagnostic/v1",
      event,
      component: "viewer",
      binding_hash: mediaDiagnosticBinding.binding_hash,
      generation: mediaDiagnosticBinding.generation,
      page_id: mediaDiagnosticBinding.page_id,
      vm_id: mediaDiagnosticBinding.vm_id,
      media_stream_id: mediaDiagnosticBinding.media_stream_id,
      channel,
      ordinal: mediaDiagnosticOrdinal++,
      ts: new Date().toISOString(),
    }));
  }

  async function emitPeerDiagnostic(event, channel, connection) {
    let stats = null;
    try {
      stats = await collectWebrtcStats(connection);
    } catch {
      // The state transition remains useful without an optional stats snapshot.
    }
    emitMediaDiagnostic(event, channel, {
      connection_state: mediaDiagnosticValue(connection?.connectionState),
      ice_connection_state: mediaDiagnosticValue(
        connection?.iceConnectionState,
      ),
      ice_gathering_state: mediaDiagnosticValue(
        connection?.iceGatheringState,
      ),
      ...mediaDiagnosticStats(stats),
    });
  }

  function metricsState() {
    return {
      latestWebrtcStats,
      latestVideoWebrtcStats,
      latestAudioWebrtcStats,
      remoteAudioExpected,
      remoteAudioUnlocked,
      remoteAudioMuted: remoteAudio.muted,
      remoteAudioPaused: remoteAudio.paused,
      remoteAudioTrackCount: Number(remoteAudio.srcObject?.getAudioTracks?.().length || 0),
      remoteAudioConnectionState: audioPeerConnection?.connectionState || "",
      remoteAudioIceConnectionState: audioPeerConnection?.iceConnectionState || "",
      remoteAudioSignalingState: audioPeerConnection?.signalingState || "",
      remoteAudioIceGatheringState: audioPeerConnection?.iceGatheringState || "",
      audioBrowserCandidateCount,
      audioEngineCandidateCount,
      audioRawCandidateEventCount,
      audioNullCandidateEventCount,
      lastAudioBrowserCandidateSummary,
      lastAudioEngineCandidateSummary,
      audioOfferSummary,
      audioAnswerSummary,
      remoteVideoMuted: remoteVideo.muted,
      remoteVideoPaused: remoteVideo.paused,
    };
  }

  function stopStatsPolling() {
    window.clearTimeout(statsTimer);
    statsTimer = 0;
    latestWebrtcStats = null;
    latestVideoWebrtcStats = null;
    latestAudioWebrtcStats = null;
  }

  function startStatsPolling(nextPeerConnection) {
    stopStatsPolling();
    if (!debugMetrics && !mediaDiagnosticBinding) {
      return;
    }
    const poll = async () => {
      if (nextPeerConnection !== peerConnection || !nextPeerConnection) {
        return;
      }
      try {
        latestVideoWebrtcStats = await collectWebrtcStats(nextPeerConnection);
        latestAudioWebrtcStats = audioPeerConnection
          ? await collectWebrtcStats(audioPeerConnection)
          : null;
        latestWebrtcStats = {
          ...(latestVideoWebrtcStats || {}),
          ...(latestAudioWebrtcStats || {}),
        };
        updateMetrics(getLastPageStatus() || {});
      } catch {
        // Metrics are diagnostic only; display health is governed by the Runtime session.
      } finally {
        statsTimer = window.setTimeout(poll, 1000);
      }
    };
    statsTimer = window.setTimeout(poll, 1000);
  }

  function close() {
    handleRemoteInputChannelTeardown();
    trackReady = false;
    failureStarted = false;
    remoteAudioExpected = false;
    remoteAudioUnlocked = false;
    engineRelayOnly = false;
    window.clearTimeout(connectTimer);
    window.clearTimeout(candidatePollTimer);
    window.clearTimeout(audioCandidatePollTimer);
    window.clearTimeout(disconnectTimer);
    window.clearTimeout(frameWatchTimer);
    candidatePollTimer = 0;
    audioCandidatePollTimer = 0;
    disconnectTimer = 0;
    frameWatchTimer = 0;
    lastVideoProgressAt = 0;
    lastVideoDecodedFrames = 0;
    lastVideoCurrentTime = 0;
    browserCandidateCount = 0;
    engineCandidateCount = 0;
    audioBrowserCandidateCount = 0;
    audioEngineCandidateCount = 0;
    audioRawCandidateEventCount = 0;
    audioNullCandidateEventCount = 0;
    lastBrowserCandidateSummary = "none";
    lastAudioBrowserCandidateSummary = "none";
    lastAudioEngineCandidateSummary = "none";
    audioOfferSummary = null;
    audioAnswerSummary = null;
    inputChannel = null;
    mediaStream = null;
    resetPageStatus();
    stopStatsPolling();
    if (peerConnection) {
      peerConnection.close();
      peerConnection = null;
    }
    if (audioPeerConnection) {
      audioPeerConnection.close();
      audioPeerConnection = null;
    }
    if (remoteVideo.srcObject) {
      for (const track of remoteVideo.srcObject.getTracks()) {
        track.stop();
      }
    }
    remoteVideo.srcObject = null;
    if (remoteAudio.srcObject) {
      for (const track of remoteAudio.srcObject.getTracks()) {
        track.stop();
      }
    }
    remoteAudio.srcObject = null;
    remoteAudio.muted = true;
    remoteAudio.defaultMuted = true;
    remoteVideo.muted = true;
    remoteVideo.defaultMuted = true;
    remoteVideo.hidden = true;
    mediaDiagnosticBinding = null;
    mediaDiagnosticOrdinal = 0;
  }

  function stopEngineCandidatePolling() {
    window.clearTimeout(candidatePollTimer);
    window.clearTimeout(audioCandidatePollTimer);
    candidatePollTimer = 0;
    audioCandidatePollTimer = 0;
  }

  function markVideoProgress() {
    lastVideoProgressAt = Date.now();
    lastVideoDecodedFrames = Number(remoteVideo.webkitDecodedFrameCount || 0);
    lastVideoCurrentTime = Number(remoteVideo.currentTime || 0);
  }

  function videoFrameProgressed() {
    const decodedFrames = Number(remoteVideo.webkitDecodedFrameCount || 0);
    const currentTime = Number(remoteVideo.currentTime || 0);
    if (
      decodedFrames > lastVideoDecodedFrames ||
      currentTime > lastVideoCurrentTime
    ) {
      lastVideoDecodedFrames = decodedFrames;
      lastVideoCurrentTime = currentTime;
      lastVideoProgressAt = Date.now();
      return true;
    }
    return false;
  }

  async function recover(message, options = {}) {
    if (failureStarted) {
      return;
    }
    failureStarted = true;
    await onRecoveryRequired(message, options);
  }

  function remoteDisplayFailureMessage(nextPeerConnection, reason) {
    const status = getLastPageStatus() || {};
    const signaling = status.webrtc_signaling || {};
    const engineCandidates = Math.max(
      Number(signaling.selkies_candidates_received || 0),
      engineCandidateCount,
    );
    const browserCandidates = Math.max(
      Number(signaling.browser_candidates_received || 0),
      browserCandidateCount,
    );
    const lastBrowserCandidate = signaling.last_browser_candidate;
    const browserCandidateSummary = lastBrowserCandidate
      ? `${lastBrowserCandidate.type || "candidate"}/${lastBrowserCandidate.address_kind || "unknown"}`
      : lastBrowserCandidateSummary;
    const mediaRouteUnavailable = engineCandidates > 0 && browserCandidates === 0;
    return [
      `Remote display negotiated but no video frame arrived (pc=${nextPeerConnection.connectionState}, ice=${nextPeerConnection.iceConnectionState}, reason=${reason}).`,
      mediaRouteUnavailable
        ? "The Browser Engine is running, but this device has no secure display relay candidate for it."
        : "The Browser Engine is running, but the secure display connection is not ready.",
      mediaRouteUnavailable
        ? "Choose a Browser Engine and Exit Node that provide a shared secure display route."
        : debugMetrics
        ? `Diagnostics: engine candidates=${engineCandidates}, browser candidates=${browserCandidates}, last browser candidate=${browserCandidateSummary}.`
        : "Runtime must close this session before another Browser Engine or Exit Node can open.",
    ].join(" ");
  }

  function malformedDisplayResponse(message) {
    const error = new Error(message);
    error.runtimeOwnedFailureKind = "malformed_response";
    return error;
  }

  function signalingFailure(message) {
    const error = new Error(message);
    error.runtimeOwnedFailureKind = "signaling";
    return error;
  }

  function requireSignalAck(response, expectedType) {
    if (
      response?.schema !== "elastos.browser.webrtc-signal-ack/v1" ||
      response?.type !== expectedType
    ) {
      throw malformedDisplayResponse(
        "Browser display signaling returned an invalid response.",
      );
    }
    return response;
  }

  function summarizeBrowserCandidate(candidate) {
    const line = String(candidate?.candidate || "");
    const tokens = line.trim().split(/\s+/);
    let type = "candidate";
    for (let index = 0; index < tokens.length - 1; index += 1) {
      if (tokens[index].toLowerCase() === "typ") {
        type = tokens[index + 1] || type;
        break;
      }
    }
    return `${type}/${tokens[2] || "unknown"}`;
  }

  function summarizeSdp(sdp) {
    const lines = String(sdp || "").split(/\r?\n/);
    return {
      bytes: new Blob([String(sdp || "")]).size,
      media: lines.filter((line) => line.startsWith("m=")).map((line) => line.slice(0, 80)),
      mids: lines.filter((line) => line.startsWith("a=mid:")).map((line) => line.slice(6, 40)),
      directions: lines
        .filter((line) =>
          line === "a=sendrecv" ||
          line === "a=sendonly" ||
          line === "a=recvonly" ||
          line === "a=inactive"
        )
        .map((line) => line.slice(2)),
      candidate_count: lines.filter((line) => line.startsWith("a=candidate:")).length,
      end_of_candidates: lines.some((line) => line === "a=end-of-candidates"),
      ice_ufrag_present: lines.some((line) => line.startsWith("a=ice-ufrag:")),
      ice_pwd_present: lines.some((line) => line.startsWith("a=ice-pwd:")),
      fingerprint_present: lines.some((line) => line.startsWith("a=fingerprint:")),
      setup: lines.find((line) => line.startsWith("a=setup:"))?.slice(8, 32) || "",
      rtpmap: lines.filter((line) => line.startsWith("a=rtpmap:")).map((line) => line.slice(9, 80)),
      fmtp: lines.filter((line) => line.startsWith("a=fmtp:")).map((line) => line.slice(7, 80)),
      rtcp_mux: lines.some((line) => line === "a=rtcp-mux"),
      rtcp_rsize: lines.some((line) => line === "a=rtcp-rsize"),
    };
  }

  function failRemoteDisplay(nextPeerConnection, reason) {
    if (nextPeerConnection !== peerConnection || failureStarted) {
      return;
    }
    failureStarted = true;
    stopEngineCandidatePolling();
    window.clearTimeout(connectTimer);
    window.clearTimeout(disconnectTimer);
    disconnectTimer = 0;
    const message = remoteDisplayFailureMessage(nextPeerConnection, reason);
    emitMediaDiagnostic("viewer_failure", "video", {
      reason: mediaDiagnosticValue(reason),
      connection_state: mediaDiagnosticValue(
        nextPeerConnection.connectionState,
      ),
      ice_connection_state: mediaDiagnosticValue(
        nextPeerConnection.iceConnectionState,
      ),
      ...mediaDiagnosticStats(latestVideoWebrtcStats),
    });
    showStatus(message, {
      sticky: true,
    });
    updateMetrics(getLastPageStatus() || {});
    Promise.resolve(
      onRecoveryRequired(message, {
        failureKind:
          reason === "no_first_frame" ? "no_first_frame" : "signaling",
        retry: reason !== "no_first_frame",
      }),
    ).catch(() => {});
  }

  function startFrameWatch(nextPeerConnection) {
    window.clearTimeout(frameWatchTimer);
    const watch = () => {
      if (
        nextPeerConnection !== peerConnection ||
        !nextPeerConnection ||
        getCurrentDisplayMode() !== "webrtc_remote_display"
      ) {
        return;
      }
      if (trackReady) {
        videoFrameProgressed();
      }
      frameWatchTimer = window.setTimeout(watch, WEBRTC_FRAME_WATCH_MS);
    };
    markVideoProgress();
    frameWatchTimer = window.setTimeout(watch, WEBRTC_FRAME_WATCH_MS);
  }

  function scheduleFailure(nextPeerConnection, reason) {
    if (nextPeerConnection !== peerConnection || failureStarted) {
      return;
    }
    if (!trackReady) {
      failRemoteDisplay(nextPeerConnection, reason);
      return;
    }
    window.clearTimeout(disconnectTimer);
    showStatus(
      `Browser display interrupted; reconnecting (${reason}).`,
      { sticky: true },
    );
    disconnectTimer = window.setTimeout(() => {
      if (
        nextPeerConnection === peerConnection &&
        getCurrentDisplayMode() === "webrtc_remote_display"
      ) {
        recover(`Browser display ${reason}; reconnecting.`, {
          failureKind: "signaling",
        }).catch(() => {});
      }
    }, reason === "failed" || reason === "closed" ? 250 : WEBRTC_DISCONNECT_GRACE_MS);
  }

  function prepareAudio(expectsAudio) {
    remoteAudioExpected = Boolean(expectsAudio);
    remoteAudioUnlocked = !remoteAudioExpected;
    // Audible autoplay is blocked by modern browsers. Start muted so the remote
    // display can render immediately, then unlock audio on the first user gesture.
    remoteVideo.muted = true;
    remoteVideo.defaultMuted = true;
    remoteVideo.volume = 1;
    remoteAudio.muted = true;
    remoteAudio.defaultMuted = true;
    remoteAudio.volume = 1;
  }

  function bindInputChannel(channel) {
    inputChannel = channel || null;
    if (!inputChannel) {
      return;
    }
    const boundInputChannel = inputChannel;
    const teardownBoundInputChannel = () => {
      if (inputChannel === boundInputChannel) {
        handleRemoteInputChannelTeardown();
      }
    };
    inputChannel.addEventListener("message", handleRemoteInputChannelMessage);
    inputChannel.addEventListener("close", teardownBoundInputChannel);
    inputChannel.addEventListener("error", teardownBoundInputChannel);
    if (inputChannel.readyState === "open") {
      emitMediaDiagnostic("viewer_input_channel_open", "input");
      return;
    }
    inputChannel.addEventListener(
      "open",
      () => {
        emitMediaDiagnostic("viewer_input_channel_open", "input");
      },
      { once: true },
    );
  }

  function hasRenderableFrame() {
    return (
      Number(remoteVideo.videoWidth || 0) > 0 &&
      Number(remoteVideo.videoHeight || 0) > 0
    );
  }

  async function unlockAudio() {
    if (!remoteAudioExpected || remoteAudioUnlocked || !remoteVideo.srcObject) {
      return;
    }
    remoteAudioUnlocked = true;
    remoteVideo.muted = false;
    remoteVideo.defaultMuted = false;
    remoteAudio.muted = false;
    remoteAudio.defaultMuted = false;
    await Promise.all([
      remoteVideo.play().catch(() => {}),
      remoteAudio.play(),
    ])
      .then(() => {
        updateMetrics(getLastPageStatus() || {});
        emitMediaDiagnostic("viewer_audio_unlocked", "audio", {
          audio_track_count: mediaDiagnosticNumber(
            remoteAudio.srcObject?.getAudioTracks?.().length,
          ),
          ...mediaDiagnosticStats(latestAudioWebrtcStats),
        });
        showStatus("Remote audio enabled.");
      })
      .catch(() => {
        remoteAudioUnlocked = false;
        remoteVideo.muted = true;
        remoteVideo.defaultMuted = true;
        remoteAudio.muted = true;
        remoteAudio.defaultMuted = true;
        updateMetrics(getLastPageStatus() || {});
        // A later user gesture will retry audio unlock; do not pin UI over the page.
      });
  }

  function unlockAudioFromGesture() {
    unlockAudio().catch(() => {});
  }

  async function applyEngineRemoteSignalsTo(nextPeerConnection, payload) {
    if (!nextPeerConnection || !payload || typeof payload !== "object") {
      return;
    }
    if (payload.candidates != null && !Array.isArray(payload.candidates)) {
      throw malformedDisplayResponse(
        "Browser display signaling returned invalid candidates.",
      );
    }
    const candidates = Array.isArray(payload.candidates)
      ? payload.candidates
      : [];
    for (const candidate of candidates) {
      const normalized = normalizeEngineCandidate(candidate);
      if (!normalized) {
        throw malformedDisplayResponse(
          "Browser display signaling returned an invalid candidate.",
        );
      }
      if (
        engineRelayOnly &&
        iceCandidateType(normalized) !== "relay"
      ) {
        throw malformedDisplayResponse(
          "Browser display signaling returned a non-relay engine candidate.",
        );
      }
      if (nextPeerConnection === peerConnection) {
        engineCandidateCount += 1;
      }
      if (nextPeerConnection === audioPeerConnection) {
        audioEngineCandidateCount += 1;
        lastAudioEngineCandidateSummary = summarizeBrowserCandidate(normalized);
        emitMediaDiagnostic("viewer_engine_candidate", "audio", {
          candidate_type: mediaDiagnosticValue(iceCandidateType(normalized)),
          candidate_count: mediaDiagnosticNumber(audioEngineCandidateCount),
        });
      } else if (nextPeerConnection === peerConnection) {
        emitMediaDiagnostic("viewer_engine_candidate", "video", {
          candidate_type: mediaDiagnosticValue(iceCandidateType(normalized)),
          candidate_count: mediaDiagnosticNumber(engineCandidateCount),
        });
      }
      await nextPeerConnection.addIceCandidate(normalized).catch(() => {});
    }
    if (payload.end_of_candidates === true) {
      await nextPeerConnection.addIceCandidate(null).catch(() => {});
    }
  }

  async function applyEngineRemoteSignals(payload) {
    await applyEngineRemoteSignalsTo(peerConnection, payload);
  }

  async function connectAudioPeer(displaySession, iceServers, iceTransportPolicy) {
    const audioOffer = displaySession.audio_offer;
    if (
      audioOffer?.schema !== "elastos.browser.webrtc-offer/v1" ||
      audioOffer?.type !== "offer" ||
      !audioOffer?.sdp ||
      (engineRelayOnly && !sdpHasOnlyRelayCandidates(audioOffer.sdp))
    ) {
      throw malformedDisplayResponse("Browser audio could not connect.");
    }
    audioOfferSummary = summarizeSdp(audioOffer.sdp);
    const nextAudioPeerConnection = new RTCPeerConnection({
      iceServers,
      iceTransportPolicy,
      bundlePolicy: "max-bundle",
      rtcpMuxPolicy: "require",
    });
    audioPeerConnection = nextAudioPeerConnection;
    emitMediaDiagnostic("viewer_peer_created", "audio");
    nextAudioPeerConnection.addTransceiver("audio", { direction: "recvonly" });
    const audioStream = new MediaStream();
    remoteAudio.srcObject = audioStream;
    const queuedAudioCandidates = [];
    let canSignalAudioCandidates = false;
    const signalAudioCandidate = async (candidate) => {
      const signalResponse = await fetchJson(displaySession.signaling_url, {
        method: "POST",
        body: candidate
          ? {
              type: "candidate",
              channel: "audio",
              candidate,
            }
          : {
              type: "end_of_candidates",
              channel: "audio",
            },
      });
      if (signalResponse?.accepted === false) {
        throw signalingFailure("Browser audio signaling was rejected.");
      }
      await applyEngineRemoteSignalsTo(
        nextAudioPeerConnection,
        requireSignalAck(
          signalResponse,
          candidate ? "candidate" : "end_of_candidates",
        ),
      );
    };
    const sendAudioCandidate = async (candidate) => {
      if (!canSignalAudioCandidates) {
        queuedAudioCandidates.push(candidate);
        return;
      }
      await signalAudioCandidate(candidate);
    };
    const pollAudioEngineCandidates = (remaining) => {
      window.clearTimeout(audioCandidatePollTimer);
      if (
        remaining <= 0 ||
        nextAudioPeerConnection !== audioPeerConnection ||
        getCurrentDisplayMode() !== "webrtc_remote_display" ||
        ["connected", "completed"].includes(nextAudioPeerConnection.iceConnectionState) ||
        ["closed", "failed"].includes(nextAudioPeerConnection.connectionState) ||
        ["closed", "failed"].includes(nextAudioPeerConnection.iceConnectionState)
      ) {
        audioCandidatePollTimer = 0;
        return;
      }
      audioCandidatePollTimer = window.setTimeout(() => {
        signalAudioCandidate(null)
          .catch((error) => {
            recover(friendlyOpenError(error), {
              failureKind:
                error.runtimeOwnedFailureKind || "signaling",
            }).catch(() => {});
          })
          .finally(() => pollAudioEngineCandidates(remaining - 1));
      }, WEBRTC_ENGINE_CANDIDATE_POLL_MS);
    };
    nextAudioPeerConnection.addEventListener("connectionstatechange", () => {
      updateMetrics(getLastPageStatus() || {});
      void emitPeerDiagnostic(
        "viewer_peer_state",
        "audio",
        nextAudioPeerConnection,
      );
    });
    nextAudioPeerConnection.addEventListener("iceconnectionstatechange", () => {
      updateMetrics(getLastPageStatus() || {});
      void emitPeerDiagnostic(
        ["connected", "completed"].includes(
          nextAudioPeerConnection.iceConnectionState,
        )
          ? "viewer_ice_pair"
          : "viewer_ice_state",
        "audio",
        nextAudioPeerConnection,
      );
    });
    nextAudioPeerConnection.addEventListener("icegatheringstatechange", () => {
      updateMetrics(getLastPageStatus() || {});
      emitMediaDiagnostic("viewer_ice_gathering", "audio", {
        ice_gathering_state: mediaDiagnosticValue(
          nextAudioPeerConnection.iceGatheringState,
        ),
        browser_candidate_count: mediaDiagnosticNumber(
          audioBrowserCandidateCount,
        ),
      });
    });
    nextAudioPeerConnection.addEventListener("icecandidate", (event) => {
      audioRawCandidateEventCount += 1;
      if (event.candidate) {
        const normalized = normalizeIceCandidateForRuntime(event.candidate.toJSON());
        if (!normalized) {
          return;
        }
        audioBrowserCandidateCount += 1;
        lastAudioBrowserCandidateSummary = summarizeBrowserCandidate(normalized);
        emitMediaDiagnostic("viewer_browser_candidate", "audio", {
          candidate_type: mediaDiagnosticValue(iceCandidateType(normalized)),
          candidate_count: mediaDiagnosticNumber(audioBrowserCandidateCount),
        });
        sendAudioCandidate(normalized).catch((error) => {
          recover(friendlyOpenError(error), {
            failureKind: error.runtimeOwnedFailureKind || "signaling",
          }).catch(() => {});
        });
        return;
      }
      audioNullCandidateEventCount += 1;
      emitMediaDiagnostic("viewer_ice_gathering_complete", "audio", {
        browser_candidate_count: mediaDiagnosticNumber(
          audioBrowserCandidateCount,
        ),
        engine_candidate_count: mediaDiagnosticNumber(
          audioEngineCandidateCount,
        ),
      });
      sendAudioCandidate(null).catch((error) => {
        recover(friendlyOpenError(error), {
          failureKind: error.runtimeOwnedFailureKind || "signaling",
        }).catch(() => {});
      });
    });
    nextAudioPeerConnection.addEventListener("track", (event) => {
      const track = event.track || null;
      if (!track || track.kind !== "audio") {
        return;
      }
      emitMediaDiagnostic("viewer_ontrack", "audio", {
        track_kind: "audio",
      });
      if (!audioStream.getTracks().some((existing) => existing.id === track.id)) {
        audioStream.addTrack(track);
      }
      updateMetrics(getLastPageStatus() || {});
      remoteAudio.play().catch(() => {});
    });
    await nextAudioPeerConnection.setRemoteDescription({
      type: "offer",
      sdp: audioOffer.sdp,
    });
    await applyEngineRemoteSignalsTo(nextAudioPeerConnection, audioOffer);
    const answer = await nextAudioPeerConnection.createAnswer();
    await nextAudioPeerConnection.setLocalDescription(answer);
    audioAnswerSummary = summarizeSdp(nextAudioPeerConnection.localDescription?.sdp || answer.sdp);
    const ack = await fetchJson(displaySession.signaling_url, {
      method: "POST",
      body: {
        type: "answer",
        channel: "audio",
        sdp: stripTrickleCandidatesFromSdp(nextAudioPeerConnection.localDescription.sdp),
      },
    });
    requireSignalAck(ack, "answer");
    await applyEngineRemoteSignalsTo(nextAudioPeerConnection, ack);
    canSignalAudioCandidates = true;
    for (const candidate of queuedAudioCandidates.splice(0)) {
      await signalAudioCandidate(candidate);
    }
    pollAudioEngineCandidates(WEBRTC_ENGINE_CANDIDATE_POLL_ATTEMPTS);
  }

  async function connect(displaySession, enginePage) {
    if (typeof RTCPeerConnection !== "function") {
      throw new Error(
        "Remote display is unavailable on this device.",
      );
    }
    if (displaySession?.schema !== "elastos.browser.display-session/v1") {
      throw malformedDisplayResponse("Browser display could not connect.");
    }
    if (displaySession.mode !== "webrtc_remote_display") {
      throw malformedDisplayResponse("Browser display is not ready.");
    }
    if (
      typeof displaySession.signaling_url !== "string" ||
      !displaySession.signaling_url.startsWith("/api/apps/browser/pages/")
    ) {
      throw malformedDisplayResponse(
        "Browser display could not establish a secure connection.",
      );
    }

    close();
    const runtimeLaunchRelayOnly =
      displaySession.ice_connection_policy === "runtime_launch_relay_only";
    engineRelayOnly = runtimeLaunchRelayOnly;
    if (
      displaySession.ice_connection_policy === "engine_relay_only" ||
      (runtimeLaunchRelayOnly &&
       (
        displaySession.offerer !== "engine" ||
        !displaySession.runtime_turn
       ))
    ) {
      throw malformedDisplayResponse(
        "Browser display returned an invalid Runtime relay policy.",
      );
    }
    trackReady = false;
    window.clearTimeout(connectTimer);
    const inputTransport =
      displaySession.input === "datachannel" ? "datachannel" : "runtime_route";
    const inputProtocol =
      displaySession.input_protocol === "selkies_v1"
        ? "selkies_v1"
        : "elastos_json";
    setDisplayInput(inputTransport, inputProtocol);
    let iceServers;
    if (runtimeLaunchRelayOnly) {
      iceServers = await validateRuntimeLaunchTurn(displaySession, enginePage).catch(() => {
        throw malformedDisplayResponse(
          "Browser display returned an invalid Runtime relay binding.",
        );
      });
      const capability = displaySession.runtime_turn;
      mediaDiagnosticBinding = {
        binding_hash: capability.binding_hash,
        generation: capability.generation,
        page_id: capability.page_id,
        vm_id: capability.vm_id,
        media_stream_id: capability.media_stream_id,
      };
    } else {
      iceServers = normalizeDisplayIceServers(displaySession.ice_servers);
    }
    const iceTransportPolicy =
      runtimeLaunchRelayOnly ||
      (displaySession.media_transport === "runtime_relay" && !engineRelayOnly)
        ? "relay"
        : "all";
    const nextPeerConnection = new RTCPeerConnection({
      iceServers,
      iceTransportPolicy,
      bundlePolicy: "max-bundle",
      rtcpMuxPolicy: "require",
    });
    startStatsPolling(nextPeerConnection);
    startFrameWatch(nextPeerConnection);
    const expectsAudio = displaySession.audio === true;
    prepareAudio(expectsAudio);
    peerConnection = nextPeerConnection;
    emitMediaDiagnostic("viewer_peer_created", "video");
    inputChannel = null;
    const offerer = displaySession.offerer === "engine" ? "engine" : "browser";
    if (inputTransport === "datachannel") {
      if (offerer === "browser") {
        bindInputChannel(
          nextPeerConnection.createDataChannel("input", { ordered: true }),
        );
      } else {
        nextPeerConnection.addEventListener("datachannel", (event) => {
          if (event.channel?.label === "input" || !inputChannel) {
            bindInputChannel(event.channel);
          }
        });
      }
    }
    if (offerer === "browser") {
      nextPeerConnection.addTransceiver("video", { direction: "recvonly" });
      if (expectsAudio) {
        nextPeerConnection.addTransceiver("audio", { direction: "recvonly" });
      }
    }
    const markReady = () => {
      if (trackReady || !remoteVideo.srcObject || !hasRenderableFrame()) {
        return;
      }
      trackReady = true;
      markVideoProgress();
      window.clearTimeout(connectTimer);
      stopEngineCandidatePolling();
      remoteVideo.hidden = false;
      renderEmpty.hidden = true;
      setActiveBrowserPage();
      emitMediaDiagnostic("viewer_first_frame", "video", {
        width: mediaDiagnosticNumber(remoteVideo.videoWidth),
        height: mediaDiagnosticNumber(remoteVideo.videoHeight),
        decoded_frames: mediaDiagnosticNumber(
          remoteVideo.webkitDecodedFrameCount,
        ),
        ...mediaDiagnosticStats(latestVideoWebrtcStats),
      });
      showStatus(
        remoteAudioExpected
          ? "Remote display ready. Click the page to enable audio."
          : "Remote display ready.",
      );
    };

    remoteVideo.addEventListener("loadeddata", markReady, { once: true });
    remoteVideo.addEventListener("loadedmetadata", markReady, { once: true });
    remoteVideo.addEventListener("canplay", markReady, { once: true });
    remoteVideo.addEventListener("resize", markReady, { once: true });
    remoteVideo.addEventListener(
      "timeupdate",
      () => {
        if (remoteVideo.currentTime > 0) {
          markReady();
        }
      },
      { once: true },
    );

    nextPeerConnection.addEventListener("track", (event) => {
      emitMediaDiagnostic("viewer_ontrack", "video", {
        track_kind: mediaDiagnosticValue(event.track?.kind, "media"),
      });
      const incomingStream = event.streams?.[0] || null;
      if (incomingStream) {
        mediaStream = incomingStream;
      } else {
        if (!mediaStream) {
          mediaStream = new MediaStream();
        }
        const hasTrack = mediaStream
          .getTracks()
          .some((track) => track.id === event.track?.id);
        if (!hasTrack && event.track) {
          mediaStream.addTrack(event.track);
        }
      }
      const stream = mediaStream || incomingStream;
      if (!stream) {
        return;
      }
      if (remoteVideo.srcObject !== stream) {
        remoteVideo.srcObject = stream;
      }
      remoteVideo.hidden = false;
      renderEmpty.hidden = true;
      if (event.track && typeof event.track.addEventListener === "function") {
        event.track.addEventListener(
          "unmute",
          () => {
            markReady();
          },
          { once: true },
        );
        event.track.addEventListener("mute", () => {
          scheduleFailure(
            nextPeerConnection,
            `${event.track.kind || "media"} track muted`,
          );
        });
        event.track.addEventListener("ended", () => {
          scheduleFailure(
            nextPeerConnection,
            `${event.track.kind || "media"} track ended`,
          );
        });
      }
      remoteVideo.play().catch(() => {});
    });
    nextPeerConnection.addEventListener("connectionstatechange", () => {
      void emitPeerDiagnostic(
        "viewer_peer_state",
        "video",
        nextPeerConnection,
      );
      if (nextPeerConnection.connectionState === "connected") {
        window.clearTimeout(disconnectTimer);
        disconnectTimer = 0;
        markVideoProgress();
        return;
      }
      if (
        ["failed", "closed", "disconnected"].includes(
          nextPeerConnection.connectionState,
        )
      ) {
        showStatus(
          `Browser remote display ${nextPeerConnection.connectionState}.`,
          {
            sticky: true,
          },
        );
        scheduleFailure(nextPeerConnection, nextPeerConnection.connectionState);
      }
    });
    nextPeerConnection.addEventListener("iceconnectionstatechange", () => {
      void emitPeerDiagnostic(
        ["connected", "completed"].includes(
          nextPeerConnection.iceConnectionState,
        )
          ? "viewer_ice_pair"
          : "viewer_ice_state",
        "video",
        nextPeerConnection,
      );
      if (
        nextPeerConnection.iceConnectionState === "connected" ||
        nextPeerConnection.iceConnectionState === "completed"
      ) {
        window.clearTimeout(disconnectTimer);
        disconnectTimer = 0;
        markVideoProgress();
        return;
      }
      if (
        ["failed", "closed", "disconnected"].includes(
          nextPeerConnection.iceConnectionState,
        )
      ) {
        scheduleFailure(
          nextPeerConnection,
          nextPeerConnection.iceConnectionState,
        );
      }
    });
    nextPeerConnection.addEventListener("icegatheringstatechange", () => {
      emitMediaDiagnostic("viewer_ice_gathering", "video", {
        ice_gathering_state: mediaDiagnosticValue(
          nextPeerConnection.iceGatheringState,
        ),
        browser_candidate_count: mediaDiagnosticNumber(browserCandidateCount),
      });
    });

    const queuedCandidates = [];
    let canSignalCandidates = false;
    const signalCandidate = async (candidate) => {
      const signalResponse = await fetchJson(displaySession.signaling_url, {
        method: "POST",
        body: candidate
          ? {
              type: "candidate",
              candidate,
            }
          : {
              type: "end_of_candidates",
            },
      });
      requireSignalAck(
        signalResponse,
        candidate ? "candidate" : "end_of_candidates",
      );
      if (signalResponse?.accepted === false) {
        throw signalingFailure("Browser display signaling was rejected.");
      }
      await applyEngineRemoteSignals(signalResponse);
    };
    const sendCandidate = async (candidate) => {
      if (!canSignalCandidates) {
        queuedCandidates.push(candidate);
        return;
      }
      await signalCandidate(candidate);
    };
    const pollEngineCandidates = (remaining) => {
      window.clearTimeout(candidatePollTimer);
      if (
        remaining <= 0 ||
        trackReady ||
        nextPeerConnection !== peerConnection ||
        getCurrentDisplayMode() !== "webrtc_remote_display" ||
        ["closed", "failed"].includes(nextPeerConnection.connectionState) ||
        ["closed", "failed"].includes(nextPeerConnection.iceConnectionState)
      ) {
        candidatePollTimer = 0;
        return;
      }
      candidatePollTimer = window.setTimeout(() => {
        signalCandidate(null)
          .catch((error) => {
            recover(friendlyOpenError(error), {
              failureKind:
                error.runtimeOwnedFailureKind || "signaling",
            }).catch(() => {});
          })
          .finally(() => pollEngineCandidates(remaining - 1));
      }, WEBRTC_ENGINE_CANDIDATE_POLL_MS);
    };
    nextPeerConnection.addEventListener("icecandidate", (event) => {
      if (event.candidate) {
        const normalized = normalizeIceCandidateForRuntime(
          event.candidate.toJSON(),
        );
        if (!normalized) {
          return;
        }
        browserCandidateCount += 1;
        lastBrowserCandidateSummary = summarizeBrowserCandidate(normalized);
        emitMediaDiagnostic("viewer_browser_candidate", "video", {
          candidate_type: mediaDiagnosticValue(iceCandidateType(normalized)),
          candidate_count: mediaDiagnosticNumber(browserCandidateCount),
        });
        sendCandidate(normalized).catch((error) => {
          recover(friendlyOpenError(error), {
            failureKind: error.runtimeOwnedFailureKind || "signaling",
          }).catch(() => {});
        });
        return;
      }
      emitMediaDiagnostic("viewer_ice_gathering_complete", "video", {
        browser_candidate_count: mediaDiagnosticNumber(browserCandidateCount),
        engine_candidate_count: mediaDiagnosticNumber(engineCandidateCount),
      });
      sendCandidate(null).catch((error) => {
        recover(friendlyOpenError(error), {
          failureKind: error.runtimeOwnedFailureKind || "signaling",
        }).catch(() => {});
      });
    });

    if (offerer === "engine") {
      const initialOffer = displaySession.initial_offer;
      if (
        initialOffer?.schema !== "elastos.browser.webrtc-offer/v1" ||
        initialOffer?.type !== "offer" ||
        !initialOffer?.sdp ||
        (engineRelayOnly &&
          !sdpHasOnlyRelayCandidates(initialOffer.sdp))
      ) {
        throw malformedDisplayResponse("Browser display could not connect.");
      }
      await nextPeerConnection.setRemoteDescription({
        type: "offer",
        sdp: initialOffer.sdp,
      });
      await applyEngineRemoteSignals(initialOffer);
      const answer = await nextPeerConnection.createAnswer();
      await nextPeerConnection.setLocalDescription(answer);
      const ack = await fetchJson(displaySession.signaling_url, {
        method: "POST",
        body: {
          type: "answer",
          sdp: stripTrickleCandidatesFromSdp(
            nextPeerConnection.localDescription.sdp,
          ),
        },
      });
      requireSignalAck(ack, "answer");
      await applyEngineRemoteSignals(ack);
    } else {
      const offer = await nextPeerConnection.createOffer();
      await nextPeerConnection.setLocalDescription(offer);
      const answer = await fetchJson(displaySession.signaling_url, {
        method: "POST",
        body: {
          type: "offer",
          sdp: stripTrickleCandidatesFromSdp(
            nextPeerConnection.localDescription.sdp,
          ),
        },
      });
      if (
        answer?.schema !== "elastos.browser.webrtc-answer/v1" ||
        answer?.type !== "answer" ||
        !answer?.sdp
      ) {
        throw malformedDisplayResponse("Browser display could not connect.");
      }
      await nextPeerConnection.setRemoteDescription({
        type: "answer",
        sdp: answer.sdp,
      });
      await applyEngineRemoteSignals(answer);
    }
    canSignalCandidates = true;
    for (const candidate of queuedCandidates.splice(0)) {
      await signalCandidate(candidate);
    }
    pollEngineCandidates(WEBRTC_ENGINE_CANDIDATE_POLL_ATTEMPTS);
    if (expectsAudio) {
      await connectAudioPeer(displaySession, iceServers, iceTransportPolicy);
    }
    connectTimer = window.setTimeout(() => {
      if (!trackReady && getCurrentDisplayMode() === "webrtc_remote_display") {
        failRemoteDisplay(nextPeerConnection, "no_first_frame");
      }
    }, WEBRTC_CONNECT_TIMEOUT_MS);
    renderPanel.focus({ preventScroll: true });
  }

  function inputChannelOpen() {
    return Boolean(inputChannel && inputChannel.readyState === "open");
  }

  function isTrackReady() {
    return trackReady;
  }

  function sendInputMessages(messages) {
    if (!inputChannelOpen()) {
      throw new Error("Browser remote-display input channel is not open.");
    }
    for (const message of messages) {
      inputChannel.send(message);
    }
    emitMediaDiagnostic("viewer_input_sent", "input", {
      message_count: mediaDiagnosticNumber(messages.length),
    });
  }

  return {
    close,
    connect,
    inputChannelOpen,
    isTrackReady,
    metricsState,
    sendInputMessages,
    unlockAudioFromGesture,
  };
}
