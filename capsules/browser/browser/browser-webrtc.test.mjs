import assert from "node:assert/strict";
import test from "node:test";

import {
  iceCandidateType,
  localAnswerSdpHasUsableCandidates,
  sdpHasOnlyRelayCandidates,
  waitForLocalAnswerIce,
} from "./browser-webrtc.js";

test("engine-only VZ media accepts only relay candidates", () => {
  const relay =
    "candidate:1 1 UDP 16777215 203.0.113.7 55001 typ relay raddr 0.0.0.0 rport 0";
  const host =
    "candidate:2 1 UDP 2122260223 192.0.2.9 49152 typ host";

  assert.equal(iceCandidateType(relay), "relay");
  assert.equal(iceCandidateType({ candidate: `a=${host}` }), "host");
  assert.equal(
    sdpHasOnlyRelayCandidates(`v=0\r\na=${relay}\r\na=end-of-candidates\r\n`),
    true,
  );
  assert.equal(
    sdpHasOnlyRelayCandidates(`v=0\r\na=${relay}\r\na=${host}\r\n`),
    false,
  );
  assert.equal(sdpHasOnlyRelayCandidates("v=0\r\n"), true);
});

function createFakePeer({ sdp = "", gathering = "new" } = {}) {
  const listeners = {
    icecandidate: new Set(),
    icegatheringstatechange: new Set(),
  };
  return {
    localDescription: { sdp },
    iceGatheringState: gathering,
    addEventListener(type, fn) {
      listeners[type].add(fn);
    },
    removeEventListener(type, fn) {
      listeners[type].delete(fn);
    },
    emit(type) {
      for (const fn of listeners[type]) {
        fn();
      }
    },
  };
}

test("relay answers wait for a relay candidate", async () => {
  const relay =
    "a=candidate:1 1 UDP 16777215 203.0.113.7 55001 typ relay raddr 0.0.0.0 rport 0";
  const host = "a=candidate:2 1 UDP 2122260223 192.0.2.9 49152 typ host";
  assert.equal(localAnswerSdpHasUsableCandidates(`v=0\r\n${host}\r\n`, true), false);
  assert.equal(localAnswerSdpHasUsableCandidates(`v=0\r\n${relay}\r\n`, true), true);
  const peer = createFakePeer();
  const waiting = waitForLocalAnswerIce(peer, { relayOnly: true, budgetMs: 1000 });
  peer.localDescription = { sdp: `v=0\r\n${host}\r\n` };
  peer.emit("icecandidate");
  peer.localDescription = { sdp: `v=0\r\n${relay}\r\n` };
  peer.emit("icecandidate");
  await waiting;
});

test("answer ICE wait ends when gathering completes", async () => {
  const peer = createFakePeer();
  const waiting = waitForLocalAnswerIce(peer, { relayOnly: true, budgetMs: 1000 });
  peer.iceGatheringState = "complete";
  peer.emit("icegatheringstatechange");
  await waiting;
});
