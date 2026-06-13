import { collectWebrtcStats } from "./browser-status.js?v=browser-20260520e";
import {
  normalizeDisplayIceServers,
  normalizeEngineCandidate,
  normalizeIceCandidateForRuntime,
  stripTrickleCandidatesFromSdp,
} from "./browser-webrtc.js?v=browser-20260520e";

const WEBRTC_CONNECT_TIMEOUT_MS = 30000;
const WEBRTC_DISCONNECT_GRACE_MS = 10000;
const WEBRTC_FRAME_WATCH_MS = 3000;

export function createBrowserRemoteDisplay({
  debugMetrics,
  fetchJson,
  friendlyOpenError,
  getCurrentDisplayMode,
  getLastPageStatus,
  handleRemoteInputChannelMessage,
  incrementFrameLoopSerial,
  onRecoveryRequired,
  remoteVideo,
  renderEmpty,
  renderImage,
  renderPanel,
  resetPageStatus,
  scheduleViewportResize,
  setActiveBrowserPage,
  setDisplayInput,
  showStatus,
  updateMetrics,
}) {
  let peerConnection = null;
  let inputChannel = null;
  let mediaStream = null;
  let trackReady = false;
  let connectTimer = 0;
  let disconnectTimer = 0;
  let frameWatchTimer = 0;
  let statsTimer = 0;
  let failureStarted = false;
  let lastVideoProgressAt = 0;
  let lastVideoDecodedFrames = 0;
  let lastVideoCurrentTime = 0;
  let latestWebrtcStats = null;
  let remoteAudioExpected = false;
  let remoteAudioUnlocked = false;

  function metricsState() {
    return {
      latestWebrtcStats,
      remoteAudioExpected,
      remoteAudioUnlocked,
    };
  }

  function stopStatsPolling() {
    window.clearTimeout(statsTimer);
    statsTimer = 0;
    latestWebrtcStats = null;
  }

  function startStatsPolling(nextPeerConnection) {
    stopStatsPolling();
    if (!debugMetrics) {
      return;
    }
    const poll = async () => {
      if (nextPeerConnection !== peerConnection || !nextPeerConnection) {
        return;
      }
      try {
        latestWebrtcStats = await collectWebrtcStats(nextPeerConnection);
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
    trackReady = false;
    failureStarted = false;
    remoteAudioExpected = false;
    remoteAudioUnlocked = false;
    window.clearTimeout(connectTimer);
    window.clearTimeout(disconnectTimer);
    window.clearTimeout(frameWatchTimer);
    disconnectTimer = 0;
    frameWatchTimer = 0;
    lastVideoProgressAt = 0;
    lastVideoDecodedFrames = 0;
    lastVideoCurrentTime = 0;
    inputChannel = null;
    mediaStream = null;
    resetPageStatus();
    stopStatsPolling();
    if (peerConnection) {
      peerConnection.close();
      peerConnection = null;
    }
    if (remoteVideo.srcObject) {
      for (const track of remoteVideo.srcObject.getTracks()) {
        track.stop();
      }
    }
    remoteVideo.srcObject = null;
    remoteVideo.muted = true;
    remoteVideo.defaultMuted = true;
    remoteVideo.hidden = true;
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

  async function recover(message) {
    if (failureStarted) {
      return;
    }
    failureStarted = true;
    await onRecoveryRequired(message);
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
    window.clearTimeout(disconnectTimer);
    showStatus(
      `Browser remote display interrupted; reconnecting through Runtime (${reason}).`,
      { sticky: true },
    );
    disconnectTimer = window.setTimeout(() => {
      if (
        nextPeerConnection === peerConnection &&
        getCurrentDisplayMode() === "webrtc_remote_display"
      ) {
        recover(`Browser remote display ${reason}; reconnecting through Runtime.`).catch(() => {});
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
  }

  function bindInputChannel(channel) {
    inputChannel = channel || null;
    if (!inputChannel) {
      return;
    }
    inputChannel.addEventListener("message", handleRemoteInputChannelMessage);
    if (inputChannel.readyState === "open") {
      scheduleViewportResize({ force: true });
      return;
    }
    inputChannel.addEventListener(
      "open",
      () => {
        scheduleViewportResize({ force: true });
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
    await remoteVideo
      .play()
      .then(() => {
        showStatus("Remote audio enabled.");
      })
      .catch(() => {
        remoteAudioUnlocked = false;
        remoteVideo.muted = true;
        remoteVideo.defaultMuted = true;
        showStatus("Click the page to enable remote audio.", { sticky: true });
      });
  }

  function unlockAudioFromGesture() {
    unlockAudio().catch(() => {});
  }

  async function applyEngineRemoteSignals(payload) {
    if (!peerConnection || !payload || typeof payload !== "object") {
      return;
    }
    const candidates = Array.isArray(payload.candidates)
      ? payload.candidates
      : [];
    for (const candidate of candidates) {
      const normalized = normalizeEngineCandidate(candidate);
      if (!normalized) {
        continue;
      }
      await peerConnection.addIceCandidate(normalized).catch(() => {});
    }
    if (payload.end_of_candidates === true) {
      await peerConnection.addIceCandidate(null).catch(() => {});
    }
  }

  async function connect(displaySession) {
    if (typeof RTCPeerConnection !== "function") {
      throw new Error(
        "This host does not provide WebRTC remote display support.",
      );
    }
    if (displaySession?.schema !== "elastos.browser.display-session/v1") {
      throw new Error(
        "Browser Engine Adapter did not return a display session.",
      );
    }
    if (displaySession.mode !== "webrtc_remote_display") {
      throw new Error(
        `Browser expected WebRTC display, got ${displaySession.mode || "none"}.`,
      );
    }
    if (
      typeof displaySession.signaling_url !== "string" ||
      !displaySession.signaling_url.startsWith("/api/apps/browser/pages/")
    ) {
      throw new Error(
        "Browser WebRTC display session is missing Runtime signaling.",
      );
    }

    close();
    incrementFrameLoopSerial();
    trackReady = false;
    window.clearTimeout(connectTimer);
    const inputTransport =
      displaySession.input === "datachannel" ? "datachannel" : "runtime_route";
    const inputProtocol =
      displaySession.input_protocol === "selkies_v1"
        ? "selkies_v1"
        : "elastos_json";
    setDisplayInput(inputTransport, inputProtocol);
    const iceServers = normalizeDisplayIceServers(displaySession.ice_servers);
    const nextPeerConnection = new RTCPeerConnection({
      iceServers,
      bundlePolicy: "max-bundle",
      rtcpMuxPolicy: "require",
    });
    startStatsPolling(nextPeerConnection);
    startFrameWatch(nextPeerConnection);
    const expectsAudio = displaySession.audio === true;
    prepareAudio(expectsAudio);
    peerConnection = nextPeerConnection;
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
      remoteVideo.hidden = false;
      renderImage.hidden = true;
      renderEmpty.hidden = true;
      setActiveBrowserPage();
      showStatus(
        remoteAudioExpected
          ? "Remote display ready. Click the page to enable audio."
          : "Remote display ready through Runtime.",
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
      if (
        candidate &&
        signalResponse?.accepted === false &&
        signalResponse?.reason
      ) {
        showStatus(
          `WebRTC candidate rejected by Browser Engine Adapter (${signalResponse.reason}).`,
          {
            sticky: true,
          },
        );
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
    nextPeerConnection.addEventListener("icecandidate", (event) => {
      if (event.candidate) {
        const normalized = normalizeIceCandidateForRuntime(
          event.candidate.toJSON(),
        );
        if (!normalized) {
          return;
        }
        sendCandidate(normalized).catch((error) => {
          showStatus(friendlyOpenError(error), { sticky: true });
        });
        return;
      }
      sendCandidate(null).catch((error) => {
        showStatus(friendlyOpenError(error), { sticky: true });
      });
    });

    if (offerer === "engine") {
      const initialOffer = displaySession.initial_offer;
      if (
        initialOffer?.schema !== "elastos.browser.webrtc-offer/v1" ||
        initialOffer?.type !== "offer" ||
        !initialOffer?.sdp
      ) {
        throw new Error(
          "Browser Engine Adapter returned an invalid engine WebRTC offer.",
        );
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
      if (
        ack?.schema !== "elastos.browser.webrtc-signal-ack/v1" ||
        ack?.type !== "answer"
      ) {
        throw new Error(
          "Browser Engine Adapter returned an invalid WebRTC answer ack.",
        );
      }
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
        throw new Error(
          "Browser Engine Adapter returned an invalid WebRTC answer.",
        );
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
    connectTimer = window.setTimeout(() => {
      if (!trackReady && getCurrentDisplayMode() === "webrtc_remote_display") {
        recover("WebRTC browser stream unavailable; reconnecting through Runtime.").catch(() => {});
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
