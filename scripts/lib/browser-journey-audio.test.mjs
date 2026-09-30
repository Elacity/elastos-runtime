import assert from "node:assert/strict";
import test from "node:test";
import vm from "node:vm";
import { controlledTonePresent, installBrowserJourneyAudioProbe } from "./browser-journey-audio.mjs";

function proof() {
  return { ok: true, receiver_unchanged: true, receiver_muted: false, receiver_paused: false,
    track_state: "live", context_state: "running", probe_context_closed: true,
    samples: Array.from({ length: 40 }, (_, i) => ({ at_ms: 500 + i * 50, rms: 0.03, peak_hz: 445.3 })) };
}
test("known decoded tone passes; transport counters alone, silence and another tone fail", () => {
  assert.equal(controlledTonePresent(proof()), true);
  assert.equal(controlledTonePresent({ ok: true, audio_bytes_received: 12345 }), false);
  for (const sample of [{ rms: 0, peak_hz: 445.3 }, { rms: 0.03, peak_hz: 880 }, { rms: NaN, peak_hz: 440 }]) {
    const value = proof(); value.samples = value.samples.map(s => ({ ...s, ...sample }));
    assert.equal(controlledTonePresent(value), false);
  }
});
test("muted, paused, replaced, ended receivers and uncleared probe contexts fail", () => {
  for (const patch of [{ receiver_muted: true }, { receiver_paused: true }, { receiver_unchanged: false },
    { track_state: "ended" }, { probe_context_closed: false }, { context_state: "suspended" }]) {
    assert.equal(controlledTonePresent({ ...proof(), ...patch }), false);
  }
  assert.equal(controlledTonePresent({ ...proof(), samples: proof().samples.slice(0, 5) }), false);
});

test("a missing product receiver creates no probe context", async () => {
  let contexts = 0;
  const window = { Audio: class {} };
  vm.runInNewContext(`(${installBrowserJourneyAudioProbe})()`, {
    window, AudioContext: class { constructor() { contexts++; } },
  });
  const result = await window.__readBrowserJourneyAudio();
  assert.equal(result.stage, "receiver_count");
  assert.equal(result.count, 0);
  assert.equal(contexts, 0);
});

test("probe failures close owned resources and preserve the product track", async () => {
  for (const failure of ["resume", "analyser"]) {
    let closed = 0, disconnected = 0, stopped = 0;
    const track = { id: "product-track", readyState: "live", stop() { stopped++; } };
    const stream = { getAudioTracks: () => [track] };
    const window = { Audio: class { srcObject = stream; } };
    class ProbeContext {
      state = "running";
      async resume() { if (failure === "resume") throw new Error("resume failed"); }
      createMediaStreamSource(received) {
        assert.equal(received, stream);
        return { disconnect() { disconnected++; } };
      }
      createAnalyser() { throw new Error("analyser failed"); }
      async close() { closed++; this.state = "closed"; }
    }
    vm.runInNewContext(`(${installBrowserJourneyAudioProbe})()`, {
      window, AudioContext: ProbeContext, performance, setTimeout, clearTimeout,
    });
    const receiver = new window.Audio();
    const result = await window.__readBrowserJourneyAudio();
    assert.equal(result.ok, false);
    assert.equal(result.error, `${failure} failed`);
    assert.equal(result.probe_context_closed, true);
    assert.equal(closed, 1);
    assert.equal(disconnected, failure === "analyser" ? 1 : 0);
    assert.equal(stopped, 0);
    assert.equal(receiver.srcObject, stream);
    assert.equal(track.readyState, "live");
  }
});

function audioProbeFixture({ stats, setup } = {}) {
  const timers = new Map();
  const state = { now: 0, nextTimer: 0, statsCalls: 0, closedContexts: 0, productClosed: 0 };
  state.setTimeout = (callback, delay) => {
    const id = ++state.nextTimer;
    timers.set(id, { at: state.now + delay, callback });
    return id;
  };
  const track = state.track = { id: "observed-track", readyState: "live",
    stop() { state.productClosed++; } };
  const stream = state.stream = { getAudioTracks: () => [track] };
  class ProductPeer {
    constructor(receivers) { this.receivers = receivers; }
    connectionState = "connected";
    getReceivers() { return this.receivers; }
    close() { state.productClosed++; }
  }
  const window = state.window = { Audio: class { srcObject = stream; muted = false; paused = false; },
    RTCPeerConnection: ProductPeer };
  class ProbeContext {
    state = "running";
    sampleRate = 48000;
    async resume() {}
    createMediaStreamSource(received) {
      assert.equal(received, stream);
      return { connect() {}, disconnect() {} };
    }
    createAnalyser() {
      return { fftSize: 2048, frequencyBinCount: 1024, disconnect() {},
        getFloatTimeDomainData(pcm) { pcm.fill(0.03); },
        getFloatFrequencyData(spectrum) { spectrum.fill(-80); spectrum[19] = -1; } };
    }
    async close() { this.state = "closed"; state.closedContexts++; }
  }
  vm.runInNewContext(`(${installBrowserJourneyAudioProbe})()`, {
    window, AudioContext: ProbeContext, performance: { now: () => state.now },
    setTimeout: state.setTimeout, clearTimeout: id => timers.delete(id),
  });
  state.audio = new window.Audio();
  state.receiver = { track, getStats() {
    state.statsCalls++;
    return stats ? stats(state) : rtpReport(state);
  } };
  state.peer = new window.RTCPeerConnection([state.receiver]);
  setup?.(state);
  state.run = async () => {
    let result, failure, done = false;
    window.__readBrowserJourneyAudio().then(value => { result = value; done = true; },
      error => { failure = error; done = true; });
    for (let steps = 0; !done && steps < 200; steps++) {
      // Drain both VM and caller microtasks before advancing the simulated clock.
      await new Promise(setImmediate);
      if (done) break;
      const next = [...timers].sort((a, b) => a[1].at - b[1].at)[0];
      assert.ok(next, "probe must finish without awaiting an unresolved stats query");
      timers.delete(next[0]);
      state.now = next[1].at;
      next[1].callback();
    }
    if (failure) throw failure;
    assert.equal(done, true, "bounded probe completed");
    assert.equal(timers.size, 0, "probe timers drained");
    assert.equal(state.closedContexts, 1);
    assert.equal(state.productClosed, 0, "product peers and tracks remain owned by Browser");
    return result;
  };
  return state;
}

function rtpReport(state, patch = {}) {
  return new Map([["private-report-id", { id: "private-report-id", type: "inbound-rtp", kind: "audio",
    trackIdentifier: state.track.id, timestamp: 100000 + state.now,
    bytesReceived: 1000 + state.now, packetsReceived: 50 + state.now / 10, packetsLost: 0,
    packetsDiscarded: 2, totalAudioEnergy: 1 + state.now / 10000,
    totalSamplesReceived: 48000 + state.now * 48, totalSamplesDuration: 1 + state.now / 1000,
    concealedSamples: state.now * 4, silentConcealedSamples: state.now * 2, concealmentEvents: 3,
    jitterBufferDelay: 0.1, jitterBufferEmittedCount: 48000,
    sdp: "PRIVATE-SDP", candidate: "PRIVATE-CANDIDATE", codecId: "PRIVATE-CODEC", ...patch }]]);
}

test("remote-outbound RTP is sampled from the same getStats report and never changes tone acceptance", async () => {
  const state = audioProbeFixture({ stats(state) {
    const reports = rtpReport(state);
    reports.set("remote-out", { id: "remote-out", type: "remote-outbound-rtp", kind: "audio",
      timestamp: 100000 + state.now, remoteTimestamp: 200000 + state.now,
      bytesSent: 2000 + state.now, packetsSent: 80 + state.now / 10,
      sdp: "PRIVATE-SDP", candidate: "PRIVATE-CANDIDATE", codecId: "PRIVATE-CODEC" });
    return reports;
  } });
  const result = await state.run();
  assert.equal(controlledTonePresent(result), true);
  const outbound = result.remote_outbound_audio_rtp;
  assert.equal(outbound.status, "observed");
  assert.equal(outbound.requests, 13);
  assert.equal(outbound.samples.length, 13);
  assert.ok(outbound.samples.at(-1).bytesSent > outbound.samples[0].bytesSent);
  assert.doesNotMatch(JSON.stringify(outbound), /PRIVATE|remote-out|codecId|candidate|sdp/);
});

test("a missing remote-outbound report stays diagnostic and keeps the 440 Hz gate unchanged", async () => {
  const state = audioProbeFixture();
  const result = await state.run();
  assert.equal(controlledTonePresent(result), true);
  assert.equal(result.remote_outbound_audio_rtp.status, "report_unavailable");
  assert.ok(result.remote_outbound_audio_rtp.samples.every(sample => !Object.hasOwn(sample, "bytesSent")));
});

test("audio ICE pair bytes come from the same getStats report and never change tone acceptance", async () => {
  const state = audioProbeFixture({ stats(state) {
    const reports = rtpReport(state);
    reports.set("pair", { id: "pair", type: "candidate-pair", state: "succeeded", nominated: true,
      bytesReceived: 8000 + state.now, bytesSent: 400 + state.now,
      packetsReceived: 40 + state.now / 10, packetsSent: 4,
      currentRoundTripTime: 0.02, availableIncomingBitrate: 120000,
      remoteCandidateId: "remote-cand", localCandidateId: "local-cand",
      ip: "203.0.113.9", address: "203.0.113.9", candidate: "PRIVATE-CANDIDATE" });
    reports.set("remote-cand", { id: "remote-cand", type: "remote-candidate", port: 49160,
      candidateType: "relay", address: "203.0.113.9", ip: "203.0.113.9",
      relatedAddress: "198.51.100.4", usernameFragment: "PRIVATE-ICE" });
    return reports;
  } });
  const result = await state.run();
  assert.equal(controlledTonePresent(result), true);
  const ice = result.audio_ice_pair;
  assert.equal(ice.status, "observed");
  assert.equal(ice.requests, 13);
  assert.equal(ice.samples.length, 13);
  assert.equal(ice.samples[0].remote_port, 49160);
  assert.equal(ice.samples[0].remote_candidate_type, "relay");
  assert.ok(ice.samples.at(-1).bytesReceived > ice.samples[0].bytesReceived);
  assert.doesNotMatch(JSON.stringify(ice), /PRIVATE|203\.0\.113|198\.51\.100|pair|remote-cand|usernameFragment|relatedAddress/);
});

test("missing or ambiguous ICE pairs stay diagnostic and keep the 440 Hz gate unchanged", async () => {
  for (const kind of ["missing", "ambiguous"]) {
    const state = audioProbeFixture({ stats(state) {
      const reports = rtpReport(state);
      if (kind === "ambiguous") {
        reports.set("pair-a", { id: "pair-a", type: "candidate-pair", state: "succeeded",
          nominated: true, bytesReceived: 1 });
        reports.set("pair-b", { id: "pair-b", type: "candidate-pair", state: "succeeded",
          nominated: true, bytesReceived: 2 });
      }
      return reports;
    } });
    const result = await state.run();
    assert.equal(controlledTonePresent(result), true);
    assert.equal(result.audio_ice_pair.status, "report_unavailable");
    assert.ok(result.audio_ice_pair.samples.every(sample => !Object.hasOwn(sample, "bytesReceived")));
  }
});

test("RTP samples use the exact observed receiver, preserve zero counters and leave PCM cadence unchanged", async () => {
  const state = audioProbeFixture({ setup(state) {
    new state.window.RTCPeerConnection([{ track: { ...state.track }, getStats() {
      assert.fail("a different track with the same ID must not be queried");
    } }]);
    state.peer.getStats = () => assert.fail("use the exact receiver, not all peer stats");
  } });
  const result = await state.run();
  assert.equal(controlledTonePresent(result), true);
  assert.equal(result.duration_ms, 2500);
  assert.deepEqual(Array.from(result.samples, sample => sample.at_ms), Array.from({ length: 50 }, (_, i) => i * 50));
  const rtp = result.inbound_audio_rtp;
  assert.equal(rtp.status, "observed");
  assert.equal(rtp.receiver_match_count, 1);
  assert.equal(rtp.requests, 13);
  assert.equal(state.statsCalls, 13);
  assert.equal(rtp.pending_at_stop, false);
  assert.deepEqual(Array.from(rtp.samples, sample => sample.at_ms), Array.from({ length: 13 }, (_, i) => i * 200));
  const first = rtp.samples[0], last = rtp.samples.at(-1);
  for (const key of ["bytesReceived", "packetsReceived", "packetsLost", "packetsDiscarded", "totalAudioEnergy",
    "totalSamplesReceived", "totalSamplesDuration", "concealedSamples", "silentConcealedSamples",
    "concealmentEvents", "jitterBufferDelay", "jitterBufferEmittedCount", "timestamp"]) {
    assert.equal(typeof first[key], "number", key);
  }
  assert.equal(first.packetsLost, 0);
  assert.ok(last.concealedSamples > first.concealedSamples);
  assert.ok(last.bytesReceived > first.bytesReceived);
  assert.equal(last.requested_at_ms, 2400);
  assert.doesNotMatch(JSON.stringify(rtp), /PRIVATE|observed-track|private-report-id|codecId|candidate|sdp/);
});

test("absent, nonnumeric and nonfinite counters remain unavailable rather than zero", async () => {
  const state = audioProbeFixture({ stats: state => rtpReport(state, {
    totalAudioEnergy: undefined, concealedSamples: null, silentConcealedSamples: "0",
    concealmentEvents: Infinity, jitterBufferDelay: NaN, packetsLost: -1,
  }) });
  const result = await state.run();
  const sample = result.inbound_audio_rtp.samples[0];
  for (const key of ["totalAudioEnergy", "concealedSamples", "silentConcealedSamples", "concealmentEvents", "jitterBufferDelay"]) {
    assert.equal(Object.hasOwn(sample, key), false, key);
  }
  assert.equal(sample.packetsLost, -1, "the signed WebRTC packet-loss counter is preserved");
  assert.equal(controlledTonePresent(result), true);
});

test("missing and ambiguous receiver bindings do not query another track or change tone acceptance", async () => {
  for (const ambiguous of [false, true]) {
    const state = audioProbeFixture({ setup(state) {
      if (ambiguous) new state.window.RTCPeerConnection([{ track: state.track, getStats() { assert.fail(); } }]);
      else state.peer.receivers = [{ track: { ...state.track }, getStats() { assert.fail(); } }];
    } });
    const result = await state.run();
    assert.equal(result.inbound_audio_rtp.status, "receiver_unavailable");
    assert.equal(result.inbound_audio_rtp.receiver_match_count, ambiguous ? 2 : 0);
    assert.equal(state.statsCalls, 0);
    assert.equal(controlledTonePresent(result), true);
  }
});

test("foreign, missing and ambiguous inbound reports remain diagnostic failures", async () => {
  for (const kind of ["foreign", "missing", "ambiguous"]) {
    const state = audioProbeFixture({ stats(state) {
      const reports = rtpReport(state, kind === "foreign" ? { trackIdentifier: "foreign" } : {});
      if (kind === "missing") reports.clear();
      if (kind === "ambiguous") reports.set("other", { ...reports.values().next().value, id: "other" });
      return reports;
    } });
    const result = await state.run();
    assert.equal(result.inbound_audio_rtp.status, "report_unavailable");
    assert.ok(result.inbound_audio_rtp.samples.every(sample => !Object.hasOwn(sample, "bytesReceived")));
    assert.equal(controlledTonePresent(result), true);
  }
});

test("receiver-scoped stats can omit trackIdentifier; changing the report ID stops accumulation", async () => {
  const state = audioProbeFixture({ stats: state => rtpReport(state, {
    trackIdentifier: undefined, id: state.statsCalls === 1 ? "first" : "replacement",
  }) });
  const result = await state.run();
  assert.equal(state.statsCalls, 2);
  assert.equal(result.inbound_audio_rtp.samples[0].status, "observed");
  assert.equal(result.inbound_audio_rtp.status, "report_changed");
  assert.equal(Object.hasOwn(result.inbound_audio_rtp.samples[1], "bytesReceived"), false);
});

test("stream, receiver, track and peer changes during getStats discard late counters", async () => {
  for (const kind of ["stream", "receiver", "track", "peer"]) {
    const state = audioProbeFixture({ stats(state) {
      return new Promise(resolve => state.setTimeout(() => {
        if (kind === "stream") state.audio.srcObject = { getAudioTracks: () => [state.track] };
        if (kind === "receiver") state.peer.receivers = [];
        if (kind === "track") state.receiver.track = { ...state.track };
        if (kind === "peer") state.peer.connectionState = "closed";
        resolve(rtpReport(state));
      }, 25));
    } });
    const result = await state.run();
    assert.equal(state.statsCalls, 1);
    assert.equal(result.inbound_audio_rtp.status, "receiver_changed", kind);
    assert.equal(result.inbound_audio_rtp.samples.length, 0);
    assert.equal(result.ok, kind !== "stream", "existing PCM stream-replacement guard is preserved");
  }
});

test("slow stats stay single-flight and record completion time without changing PCM sampling", async () => {
  const state = audioProbeFixture({ stats(state) {
    return new Promise(resolve => state.setTimeout(() => resolve(rtpReport(state)), 350));
  } });
  // The final pending native query has no observer-owned timer in a real browser.
  // Use delays that resolve before the probe ends for this controlled scheduler.
  const original = state.receiver.getStats;
  state.receiver.getStats = () => state.now >= 2000 ? rtpReport(state) : original();
  const result = await state.run();
  const rtp = result.inbound_audio_rtp;
  assert.equal(rtp.samples[0].requested_at_ms, 0);
  assert.equal(rtp.samples[0].at_ms, 350);
  assert.equal(rtp.samples[1].requested_at_ms, 350);
  assert.equal(result.samples.length, 50);
  assert.equal(result.duration_ms, 2500);
});

test("hung or rejected stats never extend the PCM window and late settlement cannot mutate evidence", async () => {
  for (const mode of ["resolve", "reject", "throw"]) {
    let settle;
    const state = audioProbeFixture({ stats() {
      if (mode === "throw") throw new Error("PRIVATE-FAILURE");
      return new Promise((resolve, reject) => { settle = mode === "resolve" ? resolve : reject; });
    } });
    const result = await state.run();
    assert.equal(result.duration_ms, 2500);
    assert.equal(controlledTonePresent(result), true);
    assert.equal(state.statsCalls, mode === "throw" ? 13 : 1);
    assert.equal(result.inbound_audio_rtp.pending_at_stop, mode !== "throw");
    const frozen = JSON.stringify(result);
    if (settle) settle(mode === "resolve" ? rtpReport(state) : new Error("PRIVATE-FAILURE"));
    await new Promise(setImmediate);
    assert.equal(JSON.stringify(result), frozen);
    assert.doesNotMatch(frozen, /PRIVATE/);
  }
});
