// Test-only observation of the product receiver. The Engine fixture generates
// the tone; this probe reads decoded PCM and never creates playback audio.
export function installBrowserJourneyAudioProbe() {
  const elements = [];
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
    const result = { ok: false, samples, track_id: track.id,
      receiver_metrics_before: window.__elastosBrowserRemoteDisplayMetrics || null };
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
      while (performance.now() - started < 2500 && samples.length < 60) {
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
