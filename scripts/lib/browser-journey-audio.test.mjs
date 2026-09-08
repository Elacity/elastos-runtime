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
