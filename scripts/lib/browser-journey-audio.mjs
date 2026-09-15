// Test-only observation of the product receiver. The Engine fixture generates
// the tone; this probe reads decoded PCM and never creates playback audio.
export function installBrowserJourneyAudioProbe(options) {
  const windowMs = Number.isFinite(options?.windowMs) ? Math.max(500, options.windowMs) : 2500;
  const maxSamples = Math.max(60, Math.floor(windowMs / 50) + 4);
  const rtpLimit = Math.max(13, Math.floor(windowMs / 200) + 1);
  const elements = [];
  const peers = [];
  const NativePeer = window.RTCPeerConnection;
  if (typeof NativePeer === "function") window.RTCPeerConnection = new Proxy(NativePeer, {
    construct(target, args, newTarget) {
      const peer = Reflect.construct(target, args, newTarget);
      peers.push(peer);
      if (peers.length > 8) peers.shift();
      return peer;
    },
  });
  const NativeAudio = window.Audio;
  window.Audio = new Proxy(NativeAudio, {
    construct(target, args) {
      const audio = Reflect.construct(target, args);
      elements.push(audio);
      if (elements.length > 4) elements.shift();
      return audio;
    },
  });
  window.__readBrowserJourneyAudio = async () => {
    const active = elements.filter(element => element.srcObject?.getAudioTracks()
      .some(track => track.readyState === "live"));
    if (active.length !== 1) return { ok: false, stage: "receiver_count", count: active.length };
    const audio = active[0], stream = audio.srcObject;
    const track = stream.getAudioTracks()[0];
    const context = new AudioContext();
    let source, analyser, resumeTimer;
    const samples = [], started = performance.now();
    const result = { ok: false, samples, track_id: track.id, window_ms: windowMs,
      receiver_metrics_before: window.__elastosBrowserRemoteDisplayMetrics || null };
    const rtp = result.inbound_audio_rtp = { status: "receiver_unavailable", receiver_match_count: 0,
      requests: 0, samples: [] };
    const outbound = result.remote_outbound_audio_rtp = { status: "receiver_unavailable",
      requests: 0, samples: [] };
    const ice = result.audio_ice_pair = { status: "receiver_unavailable",
      requests: 0, samples: [] };
    let binding, rtpStopped = false, rtpPending = false, nextRtpAt = 0, reportId;
    try {
      const matches = peers.flatMap(peer => peer.connectionState === "closed" ? [] :
        peer.getReceivers().filter(receiver => receiver.track === track).map(receiver => ({ peer, receiver })));
      rtp.receiver_match_count = matches.length;
      if (matches.length === 1 && stream.getAudioTracks().length === 1 &&
          typeof matches[0].receiver.getStats === "function") {
        binding = matches[0];
        rtp.status = "observing";
      }
    } catch { rtp.status = "receiver_lookup_failed"; }
    const receiverCurrent = () => audio.srcObject === stream && track.readyState === "live" &&
      stream.getAudioTracks().length === 1 && stream.getAudioTracks()[0] === track &&
      binding.peer.connectionState !== "closed" && binding.receiver.track === track &&
      binding.peer.getReceivers().includes(binding.receiver);
    // Start at most one query every 200 ms. Never await stats on the PCM path:
    // a stalled getStats must neither stretch the probe nor alter tone evidence.
    const sampleRtp = async () => {
      const requestedAt = performance.now() - started;
      if (!binding || rtpStopped || rtpPending || requestedAt < nextRtpAt ||
          requestedAt >= windowMs || rtp.requests >= rtpLimit) return;
      nextRtpAt = requestedAt + 200;
      rtp.requests++;
      rtpPending = true;
      try {
        if (!receiverCurrent()) { rtp.status = "receiver_changed"; binding = null; return; }
        const reports = await binding.receiver.getStats();
        if (rtpStopped || performance.now() - started >= windowMs) return;
        if (!receiverCurrent()) { rtp.status = "receiver_changed"; binding = null; return; }
        const inbound = [...reports.values()].filter(item => item.type === "inbound-rtp" &&
          (item.kind === "audio" || item.mediaType === "audio"));
        const remoteOut = [...reports.values()].filter(item => item.type === "remote-outbound-rtp" &&
          (item.kind === "audio" || item.mediaType === "audio"));
        const pairs = [...reports.values()].filter(item => item.type === "candidate-pair" &&
          item.state === "succeeded" && item.nominated === true);
        const sample = { requested_at_ms: requestedAt, at_ms: performance.now() - started };
        const sent = { requested_at_ms: requestedAt, at_ms: sample.at_ms };
        const iceSample = { requested_at_ms: requestedAt, at_ms: sample.at_ms };
        const item = inbound[0];
        if (inbound.length !== 1 || (item.trackIdentifier !== undefined && item.trackIdentifier !== track.id)) {
          sample.status = "report_unavailable";
          sent.status = "report_unavailable";
          iceSample.status = "report_unavailable";
        } else if (reportId !== undefined && item.id !== reportId) {
          sample.status = "report_changed";
          sent.status = "report_changed";
          iceSample.status = "report_changed";
          binding = null;
        } else {
          reportId = item.id;
          sample.status = "observed";
          // Receiver.getStats scopes reports to this exact track. Keep report IDs,
          // codecs, candidates and all other nonnumeric fields private.
          for (const key of ["timestamp", "bytesReceived", "packetsReceived", "packetsLost", "packetsDiscarded",
            "totalAudioEnergy", "totalSamplesReceived", "totalSamplesDuration", "concealedSamples",
            "silentConcealedSamples", "concealmentEvents", "jitterBufferDelay", "jitterBufferEmittedCount"]) {
            if (typeof item[key] === "number" && Number.isFinite(item[key])) sample[key] = item[key];
          }
          if (remoteOut.length !== 1) {
            sent.status = "report_unavailable";
          } else {
            const remote = remoteOut[0];
            sent.status = "observed";
            for (const key of ["timestamp", "remoteTimestamp", "bytesSent", "packetsSent"]) {
              if (typeof remote[key] === "number" && Number.isFinite(remote[key])) sent[key] = remote[key];
            }
          }
          if (pairs.length !== 1) {
            iceSample.status = "report_unavailable";
          } else {
            const pair = pairs[0];
            iceSample.status = "observed";
            for (const key of ["bytesReceived", "bytesSent", "packetsReceived", "packetsSent",
              "currentRoundTripTime", "availableIncomingBitrate"]) {
              if (typeof pair[key] === "number" && Number.isFinite(pair[key])) iceSample[key] = pair[key];
            }
            const remoteCandidate = [...reports.values()].find(entry =>
              entry.type === "remote-candidate" && entry.id === pair.remoteCandidateId);
            if (Number.isInteger(remoteCandidate?.port) && remoteCandidate.port > 0 &&
                remoteCandidate.port <= 65535) {
              iceSample.remote_port = remoteCandidate.port;
            }
            if (typeof remoteCandidate?.candidateType === "string" &&
                remoteCandidate.candidateType.length <= 16) {
              iceSample.remote_candidate_type = remoteCandidate.candidateType;
            }
          }
        }
        rtp.samples.push(sample);
        rtp.status = sample.status;
        outbound.samples.push(sent);
        outbound.status = sent.status;
        outbound.requests = rtp.requests;
        ice.samples.push(iceSample);
        ice.status = iceSample.status;
        ice.requests = rtp.requests;
      } catch {
        if (!rtpStopped && performance.now() - started < windowMs) rtp.status = "get_stats_failed";
      } finally { rtpPending = false; }
    };
    try {
      await Promise.race([context.resume(), new Promise((_, reject) => {
        resumeTimer = setTimeout(() => reject(new Error("probe context resume timeout")), 3000);
      })]);
      clearTimeout(resumeTimer);
      source = context.createMediaStreamSource(stream);
      analyser = context.createAnalyser();
      analyser.fftSize = 2048;
      analyser.smoothingTimeConstant = 0;
      source.connect(analyser);
      const pcm = new Float32Array(analyser.fftSize);
      const spectrum = new Float32Array(analyser.frequencyBinCount);
      while (performance.now() - started < windowMs && samples.length < maxSamples) {
        if (audio.srcObject !== stream || track.readyState !== "live") {
          throw new Error("product audio receiver changed");
        }
        analyser.getFloatTimeDomainData(pcm);
        analyser.getFloatFrequencyData(spectrum);
        let energy = 0, peak = 0;
        for (const value of pcm) energy += value * value;
        for (let i = 1; i < spectrum.length; i++) if (spectrum[i] > spectrum[peak]) peak = i;
        samples.push({ at_ms: performance.now() - started, rms: Math.sqrt(energy / pcm.length),
          peak_hz: peak * context.sampleRate / analyser.fftSize });
        void sampleRtp();
        await new Promise(resolve => setTimeout(resolve, 50));
      }
      Object.assign(result, { ok: true, duration_ms: performance.now() - started,
        sample_rate: context.sampleRate,
        receiver_metrics_after: window.__elastosBrowserRemoteDisplayMetrics || null,
        receiver_muted: audio.muted,
        receiver_paused: audio.paused, context_state: context.state,
        track_state: track.readyState, receiver_unchanged: audio.srcObject === stream });
    } catch (error) {
      Object.assign(result, { stage: "decoded_audio", error: error.message });
    } finally {
      rtpStopped = true;
      rtp.pending_at_stop = rtpPending;
      clearTimeout(resumeTimer);
      source?.disconnect();
      analyser?.disconnect();
      await context.close();
      result.probe_context_closed = context.state === "closed";
    }
    return result;
  };
}

export function controlledTonePresent(result) {
  const samples = result?.samples?.filter(sample => sample.at_ms >= 500) || [];
  return result?.ok === true && result.receiver_unchanged === true &&
    result.receiver_muted === false && result.receiver_paused === false &&
    result.track_state === "live" && result.context_state === "running" &&
    result.probe_context_closed === true && samples.length >= 20 &&
    samples.filter(sample => Number.isFinite(sample.rms) && sample.rms > 0.003 &&
      Number.isFinite(sample.peak_hz) && Math.abs(sample.peak_hz - 440) <= 40).length >= samples.length * 0.9;
}
