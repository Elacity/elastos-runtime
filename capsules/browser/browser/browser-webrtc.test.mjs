import assert from "node:assert/strict";
import test from "node:test";

import {
  iceCandidateType,
  sdpHasOnlyRelayCandidates,
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
