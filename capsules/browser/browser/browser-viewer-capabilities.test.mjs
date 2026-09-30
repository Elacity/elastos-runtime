import assert from "node:assert/strict";
import test from "node:test";
import { browserViewerCapabilities, requireBrowserViewer } from "./browser-runtime-api.js";
import { friendlyOpenError } from "./browser-status.js";

function hostFixture() {
  class Peer {
    constructor() { throw new Error("Capability inspection must not allocate a peer"); }
    addTransceiver() {}
    createDataChannel() {}
  }
  return {
    RTCPeerConnection: Peer,
    RTCRtpReceiver: { getCapabilities: (kind) => ({
      codecs: [{ mimeType: kind === "video" ? "video/H264" : "audio/opus" }],
    }) },
    fetch() { throw new Error("Capability inspection must not dispatch requests"); },
    navigator: { mediaDevices: {
      getUserMedia() { throw new Error("Receive-only inspection must not request device access"); },
    } },
  };
}

test("viewer eligibility uses capabilities without allocating media or requesting devices", () => {
  const report = requireBrowserViewer("webrtc_remote_display", hostFixture());
  assert.equal(report.eligible, true);
  assert.deepEqual(report.video_codecs, ["video/h264"]);
  assert.deepEqual(report.audio_codecs, ["audio/opus"]);
  assert.equal(report.schema, "elastos.browser.viewer-capabilities/v1");
});

test("missing transport or receive codecs produces a typed pre-effect denial", () => {
  const cases = [
    (host) => { delete host.RTCPeerConnection; },
    (host) => { delete host.RTCPeerConnection.prototype.addTransceiver; },
    (host) => { delete host.RTCPeerConnection.prototype.createDataChannel; },
    (host) => { host.RTCRtpReceiver.getCapabilities = () => ({ codecs: [] }); },
    (host) => { host.RTCRtpReceiver.getCapabilities = () => { throw new Error("disabled"); }; },
  ];
  for (const change of cases) {
    const host = hostFixture();
    change(host);
    assert.throws(() => requireBrowserViewer("webrtc_remote_display", host), (error) => {
      assert.equal(error.payload.code, "viewer_unavailable");
      assert.equal(error.payload.stage, "viewer_compatibility");
      assert.deepEqual(error.payload.outcome.effects, {
        page_acquired: false, vm_acquired: false, stream_acquired: false,
      });
      assert.match(friendlyOpenError(error), /supported browser/);
      return true;
    });
  }
});

test("an unavailable codec query stays unknown until actual negotiation", () => {
  const host = hostFixture();
  delete host.RTCRtpReceiver;
  const report = browserViewerCapabilities(host);
  assert.equal(report.eligible, true);
  assert.equal(report.video_codecs, null);
  assert.equal(report.audio_codecs, null);
});

test("a native Engine capability does not imply the web viewer can display it", () => {
  assert.throws(() => requireBrowserViewer("native_surface", hostFixture()), (error) => {
    assert.equal(error.payload.code, "unsupported_viewer_display_mode");
    assert.match(friendlyOpenError(error), /streamed display/);
    return true;
  });
});
