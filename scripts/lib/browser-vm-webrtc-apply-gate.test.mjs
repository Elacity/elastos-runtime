import assert from "node:assert/strict";
import test from "node:test";

import {
  createGuestWebrtcApplyGate,
  webrtcSignalKind,
} from "./browser-vm-webrtc-apply-gate.mjs";

test("classifies attach, answer, and ICE signals", () => {
  assert.equal(
    webrtcSignalKind({ signal: { type: "display_attach" } }),
    "display_attach",
  );
  assert.equal(webrtcSignalKind({ signal: { type: "answer" } }), "answer");
  assert.equal(webrtcSignalKind({ signal: { type: "candidate" } }), "ice");
  assert.equal(
    webrtcSignalKind({ signal: { type: "end_of_candidates" } }),
    "ice",
  );
  assert.equal(webrtcSignalKind({ signal: { type: "offer" } }), "other");
});

test("holds ICE until the same-channel answer is applied", async () => {
  const gate = createGuestWebrtcApplyGate();
  const order = [];
  const forward = async (body) => {
    order.push(body.signal.type);
    return { type: body.signal.type };
  };
  const ice = gate.apply(
    "page:one",
    { channel: "video", signal: { type: "candidate" } },
    forward,
  );
  const answer = gate.apply(
    "page:one",
    { channel: "video", signal: { type: "answer" } },
    forward,
  );
  assert.deepEqual(order, ["answer"]);
  await answer;
  await ice;
  assert.deepEqual(order, ["answer", "candidate"]);
});

test("keeps audio ICE independent of the video answer", async () => {
  const gate = createGuestWebrtcApplyGate();
  const order = [];
  const forward = async (body) => {
    order.push(`${body.channel}:${body.signal.type}`);
    return { type: body.signal.type };
  };
  const audioIce = gate.apply(
    "page:one",
    { channel: "audio", signal: { type: "candidate" } },
    forward,
  );
  await gate.apply(
    "page:one",
    { channel: "video", signal: { type: "answer" } },
    forward,
  );
  assert.equal(order.join(","), "video:answer");
  await gate.apply(
    "page:one",
    { channel: "audio", signal: { type: "answer" } },
    forward,
  );
  await audioIce;
  assert.deepEqual(order, [
    "video:answer",
    "audio:answer",
    "audio:candidate",
  ]);
});

test("forwards ICE from a previous display generation", async () => {
  const gate = createGuestWebrtcApplyGate();
  const order = [];
  const forward = async (body) => {
    order.push(body.signal.type);
    if (body.signal.type === "display_attach") {
      return { display_generation: "display:new" };
    }
    return { type: body.signal.type };
  };
  await gate.apply(
    "page:one",
    { signal: { type: "display_attach" } },
    forward,
  );
  await gate.apply(
    "page:one",
    {
      channel: "video",
      signal: { type: "candidate", display_generation: "display:old" },
    },
    forward,
  );
  assert.deepEqual(order, ["display_attach", "candidate"]);
});

test("drops held ICE when display_attach starts a new pair", async () => {
  const gate = createGuestWebrtcApplyGate();
  const ice = gate.apply(
    "page:one",
    { channel: "video", signal: { type: "candidate" } },
    async () => ({ ok: true }),
  );
  await gate.apply(
    "page:one",
    { signal: { type: "display_attach" } },
    async () => ({ attached: true }),
  );
  await assert.rejects(ice, /replaced pending ICE/);
});
