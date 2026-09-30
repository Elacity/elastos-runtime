import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
const source = readFileSync(new URL('../capsules/browser/browser/browser-remote-display.js', import.meta.url), 'utf8');
function declaration(name) {
  const start = source.search(new RegExp(`  (?:async )?function ${name}\\(`));
  const end = source.indexOf('\n  }', start) + 4;
  assert.ok(start >= 0 && end > start); return source.slice(start, end);
}
function fixture() {
  const calls = [], state = vm.createContext({ supported: true, peerConnection: {}, audioPeerConnection: null, viewerOwnerActive: () => true,
    supportsDisplayGeneration: () => state.supported,
    malformedDisplayResponse: message => new Error(message), signalingFailure: message => new Error(message),
    fetchJson: async (url, options) => { calls.push({ url, body: options.body }); return { accepted: true, display_generation: options.body.display_generation }; },
  });
  for (const name of ['displayForRuntimeSignaling', 'isCurrentDisplayPeer', 'sendRuntimeSignal']) vm.runInContext(declaration(name), state);
  const display = { signaling_url: '/api/apps/browser/pages/page-one/webrtc', display_generation: 'display:' + 'a'.repeat(32) };
  return { state, display, calls };
}
for (const type of ['answer', 'candidate', 'end_of_candidates']) for (const channel of ['video', 'audio']) {
  test(`${channel} ${type} binds request and response to the captured display`, async () => {
    const f = fixture(), display = f.state.displayForRuntimeSignaling(f.display);
    f.display.display_generation = 'display:' + 'b'.repeat(32); f.state.supported = false;
    const result = await f.state.sendRuntimeSignal(f.state.peerConnection, display, { method: 'POST', body: { type, channel } });
    assert.equal(f.calls[0].body.display_generation, 'display:' + 'a'.repeat(32));
    assert.equal(result.display_generation, f.calls[0].body.display_generation);
  });
}
for (const missing of ['runtime-capability', 'engine-generation']) test(`${missing} preserves initial legacy request shape`, async () => {
  const f = fixture(); if (missing === 'runtime-capability') f.state.supported = false; else delete f.display.display_generation;
  const display = f.state.displayForRuntimeSignaling(f.display); f.state.supported = true;
  await f.state.sendRuntimeSignal(f.state.peerConnection, display, { method: 'POST', body: { type: 'answer' } });
  assert.equal(Object.hasOwn(f.calls[0].body, 'display_generation'), false);
});
for (const generation of [undefined, 'display:' + 'b'.repeat(32)]) test(`missing or stale response generation is rejected before candidate application: ${generation === undefined}`, async () => {
  const f = fixture(), wait = {};
  f.state.fetchJson = async () => new Promise(resolve => { wait.resolve = resolve; });
  const pending = f.state.sendRuntimeSignal(f.state.peerConnection, f.state.displayForRuntimeSignaling(f.display), { body: { type: 'answer' } });
  wait.resolve({ accepted: true, display_generation: generation });
  await assert.rejects(pending, /generation changed/);
});
test('malformed generation is rejected before dispatch when Runtime supports it', () => {
  const f = fixture(); f.display.display_generation = 'bad';
  assert.throws(() => f.state.displayForRuntimeSignaling(f.display), /generation is invalid/);
  assert.deepEqual(f.calls, []);
});

function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const flush = () => new Promise(resolve => setImmediate(resolve));
const generation = letter => 'display:' + letter.repeat(32);
const ice = label => ({ candidate: `candidate:${label} 1 udp 1 127.0.0.1 9000 typ relay`, sdpMLineIndex: 0 });

// Execute the actual controller and its nested callbacks. Only browser/platform
// objects are replaced; requests, candidate application and recovery use source.
function controllerFixture({ audio = true, offerer = 'engine' } = {}) {
  class Events {
    listeners = new Map();
    addEventListener(type, callback) {
      this.listeners.set(type, [...(this.listeners.get(type) || []), callback]);
    }
    emit(type, event = {}) { for (const callback of this.listeners.get(type) || []) callback(event); }
  }
  class Media extends Events {
    srcObject = null; muted = true; hidden = true; currentTime = 0;
    videoWidth = 0; videoHeight = 0; webkitDecodedFrameCount = 0;
    play() { return Promise.resolve(); }
    pause() {}
  }
  class Stream {
    tracks = [];
    getTracks() { return this.tracks; }
    getAudioTracks() { return this.tracks.filter(track => track.kind === 'audio'); }
    addTrack(track) { this.tracks.push(track); }
  }
  const peers = [], calls = [], recovered = [], focused = [], timers = new Map();
  const state = { supported: true, active: true, owner: 1, fetch: null, validation: async () => [], description: null };
  class Peer extends Events {
    connectionState = 'new'; iceConnectionState = 'new'; iceGatheringState = 'complete'; signalingState = 'stable';
    added = []; descriptions = [];
    constructor() { super(); peers.push(this); }
    addTransceiver() {}
    async setRemoteDescription(value) { this.descriptions.push(value); if (state.description) await state.description(this); }
    async setLocalDescription(value) {
      this.localDescription = value;
      if (state.localIce) {
        for (const candidate of state.localIce) {
          this.emit('icecandidate', { candidate: { toJSON: () => candidate } });
        }
      }
    }
    async createAnswer() { return { type: 'answer', sdp: 'v=0\r\ns=answer\r\n' }; }
    async createOffer() { return { type: 'offer', sdp: 'v=0\r\ns=offer\r\n' }; }
    async addIceCandidate(value) { this.added.push(value); }
    close() { this.connectionState = 'closed'; this.signalingState = 'closed'; }
  }
  let timerId = 0;
  const remoteVideo = new Media();
  const context = vm.createContext({ Audio: Media, MediaStream: Stream, RTCPeerConnection: Peer, Blob,
    console: { info() {} },
    window: { setTimeout(callback, delay) { timers.set(++timerId, { callback, delay }); return timerId; },
      clearTimeout(id) { timers.delete(id); } },
    collectWebrtcStats: async () => ({}), iceCandidateType: () => 'relay',
    normalizeDisplayIceServers: value => value || [], normalizeEngineCandidate: value => value,
    normalizeIceCandidateForRuntime: value => value, sdpHasOnlyRelayCandidates: () => true,
    stripTrickleCandidatesFromSdp: value => value,
    waitForLocalAnswerIce: async () => {},
    validateRuntimeLaunchTurn: (...args) => state.validation(...args),
  });
  vm.runInContext(source.slice(source.indexOf('const WEBRTC_CONNECT_TIMEOUT_MS')).replace('export function createBrowserRemoteDisplay', 'function createBrowserRemoteDisplay'), context);
  const ack = body => body.type === 'offer'
    ? { schema: 'elastos.browser.webrtc-answer/v1', type: 'answer', sdp: 'v=0\r\ns=remote\r\n', display_generation: body.display_generation }
    : { schema: 'elastos.browser.webrtc-signal-ack/v1', type: body.type, accepted: true, display_generation: body.display_generation, candidates: [] };
  const controller = context.createBrowserRemoteDisplay({ debugMetrics: false,
    fetchJson: async (url, options) => { calls.push({ url, body: options.body }); return state.fetch ? state.fetch(options.body, ack) : ack(options.body); },
    friendlyOpenError: error => error.message, getCurrentDisplayMode: () => 'webrtc_remote_display',
    getLastPageStatus: () => ({}), supportsDisplayGeneration: () => state.supported,
    captureViewerOwnerGuard: () => { const owner = state.owner; return () => state.active && state.owner === owner; },
    handleRemoteInputChannelMessage() {}, onRecoveryRequired: async (...args) => { recovered.push(args); },
    remoteVideo, renderEmpty: {}, renderPanel: { focus() { focused.push(true); } },
    resetPageStatus() {}, setActiveBrowserPage() {}, setDisplayInput() {}, showStatus() {}, updateMetrics() {},
  });
  function display(letter) { return {
    schema: 'elastos.browser.display-session/v1', mode: 'webrtc_remote_display', offerer, audio,
    signaling_url: '/api/apps/browser/pages/page-one/webrtc', display_generation: generation(letter),
    initial_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'v=0\r\nm=video\r\n' },
    audio_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'v=0\r\nm=audio\r\n' },
  }; }
  return { controller, display, peers, calls, recovered, focused, timers, state, ack,
    pollIds: () => [...timers].filter(([, timer]) => timer.delay === 300).map(([id]) => id) };
}

for (const channel of ['video', 'audio']) for (const outcome of ['success', 'failure']) {
  test(`actual ${channel} candidate ${outcome} after replacement leaves new peer and timers intact`, async () => {
    const f = controllerFixture(); await f.controller.connect(f.display('a'), {});
    const old = f.peers[channel === 'video' ? 0 : 1], pending = deferred(), started = deferred();
    f.state.fetch = (body, ack) => {
      if (body.display_generation === generation('a') && (body.channel || 'video') === channel && body.type === 'candidate') {
        started.resolve(); return pending.promise;
      }
      return ack(body);
    };
    old.emit('icecandidate', { candidate: { toJSON: () => ice('old') } }); await started.promise;
    await f.controller.connect(f.display('b'), {});
    const polls = f.pollIds();
    if (outcome === 'success') pending.resolve({ ...f.ack({ type: 'candidate', display_generation: generation('a') }), candidates: [ice('late')] });
    else pending.reject(new Error('old signaling failed'));
    await flush();
    assert.deepEqual(f.recovered, []);
    assert.ok(f.peers.every(peer => peer.added.length === 0));
    assert.deepEqual(f.pollIds(), polls);
    const count = f.calls.length;
    old.emit('icecandidate', { candidate: { toJSON: () => ice('stale') } });
    old.emit('icecandidate', { candidate: null }); await flush();
    assert.equal(f.calls.length, count, 'retired event handlers cannot dispatch');
    f.controller.close();
  });
}

for (const channel of ['video', 'audio']) for (const outcome of ['success', 'failure']) {
  test(`actual ${channel} candidate poll ${outcome} cannot cancel replacement polling`, async () => {
    const f = controllerFixture(); await f.controller.connect(f.display('a'), {});
    const pending = deferred(), started = deferred();
    f.state.fetch = (body, ack) => {
      if (body.display_generation === generation('a') && (body.channel || 'video') === channel && body.type === 'end_of_candidates') {
        started.resolve(); return pending.promise;
      }
      return ack(body);
    };
    for (const id of f.pollIds()) f.timers.get(id).callback();
    await started.promise;
    await f.controller.connect(f.display('b'), {}); const polls = f.pollIds();
    if (outcome === 'success') pending.resolve(f.ack({ type: 'end_of_candidates', display_generation: generation('a') }));
    else pending.reject(new Error('old poll failed'));
    await flush();
    assert.deepEqual(f.recovered, []); assert.deepEqual(f.pollIds(), polls);
    f.controller.close();
  });
}

for (const channel of ['video', 'audio']) for (const outcome of ['success', 'failure']) {
  test(`actual delayed ${channel} answer ${outcome} stops old connect continuation`, async () => {
    const f = controllerFixture(), pending = deferred(), started = deferred();
    f.state.fetch = (body, ack) => {
      if (body.display_generation === generation('a') && (body.channel || 'video') === channel && body.type === 'answer') {
        started.resolve(); return pending.promise;
      }
      return ack(body);
    };
    const connecting = f.controller.connect(f.display('a'), {}); await started.promise;
    await f.controller.connect(f.display('b'), {});
    const count = f.peers.length, focused = f.focused.length, polls = f.pollIds();
    if (outcome === 'success') pending.resolve({ ...f.ack({ type: 'answer', display_generation: generation('a') }), candidates: [ice('late-answer')] });
    else pending.reject(new Error('old answer failed'));
    await connecting;
    assert.equal(f.peers.length, count, 'old video continuation cannot create an audio peer');
    assert.equal(f.focused.length, focused); assert.deepEqual(f.pollIds(), polls);
    assert.ok(f.peers.every(peer => peer.added.length === 0)); assert.deepEqual(f.recovered, []);
    f.controller.close();
  });
}

test('actual active peer still receives candidates and reports signaling errors', async () => {
  const f = controllerFixture(); await f.controller.connect(f.display('a'), {});
  f.state.fetch = (body, ack) => ({ ...ack(body), candidates: [ice('current')] });
  f.peers[0].emit('icecandidate', { candidate: { toJSON: () => ice('outgoing') } }); await flush();
  assert.equal(f.peers[0].added.length, 1);
  f.state.fetch = async () => { throw new Error('current request failed'); };
  f.peers[0].emit('icecandidate', { candidate: null }); await flush();
  assert.equal(f.recovered.length, 1); f.controller.close();
});

for (const channel of ['video', 'audio']) test(`actual current legacy ${channel} peer rejects an empty signaling response`, async () => {
  const f = controllerFixture(); f.state.supported = false;
  await f.controller.connect(f.display('a'), {});
  f.state.fetch = async () => null;
  f.peers[channel === 'video' ? 0 : 1].emit('icecandidate', { candidate: null }); await flush();
  assert.equal(f.recovered.length, 1);
  assert.equal(f.recovered[0][1].failureKind, 'malformed_response');
  f.controller.close();
});

for (const outcome of ['success', 'failure']) test(`actual delayed browser offer ${outcome} cannot resume an old viewer`, async () => {
  const f = controllerFixture({ audio: false, offerer: 'browser' }), pending = deferred(), started = deferred();
  f.state.fetch = (body, ack) => {
    if (body.display_generation === generation('a') && body.type === 'offer') { started.resolve(); return pending.promise; }
    return ack(body);
  };
  const connecting = f.controller.connect(f.display('a'), {}); await started.promise;
  await f.controller.connect(f.display('b'), {}); const polls = f.pollIds();
  if (outcome === 'success') pending.resolve({ ...f.ack({ type: 'offer', display_generation: generation('a') }), candidates: [ice('old-offer')] });
  else pending.reject(new Error('old offer failed'));
  await connecting;
  assert.equal(f.peers[0].descriptions.length, 0);
  assert.equal(f.peers[1].descriptions.length, 1);
  assert.ok(f.peers.every(peer => peer.added.length === 0));
  assert.equal(f.focused.length, 1); assert.deepEqual(f.pollIds(), polls); assert.deepEqual(f.recovered, []);
  f.controller.close();
});

for (const outcome of ['success', 'failure']) test(`TURN validation ${outcome} after replacement cannot create an old peer`, async () => {
  const f = controllerFixture(), pending = deferred(), started = deferred();
  const display = { ...f.display('a'), ice_connection_policy: 'runtime_launch_relay_only', runtime_turn: {} };
  f.state.validation = async () => { started.resolve(); return pending.promise; };
  const connecting = f.controller.connect(display, {}); await started.promise;
  await f.controller.connect(f.display('b'), {}); const count = f.calls.length, peers = f.peers.length;
  if (outcome === 'success') pending.resolve([]); else pending.reject(new Error('old grant validation failed'));
  await connecting;
  assert.equal(f.peers.length, peers); assert.equal(f.calls.length, count); assert.equal(f.focused.length, 1);
  assert.deepEqual(f.recovered, []); f.controller.close();
});

for (const channel of ['video', 'audio']) for (const outcome of ['success', 'failure']) {
  test(`closing retained owner ignores actual ${channel} candidate ${outcome} and later callbacks`, async () => {
    const f = controllerFixture(); await f.controller.connect(f.display('a'), {});
    const peer = f.peers[channel === 'video' ? 0 : 1], pending = deferred(), started = deferred();
    f.state.fetch = body => { started.resolve(); return pending.promise; };
    peer.emit('icecandidate', { candidate: { toJSON: () => ice('pending-close') } }); await started.promise;
    f.state.active = false; // The same current page and peers remain until terminal cleanup.
    if (outcome === 'success') pending.resolve({ ...f.ack({ type: 'candidate', display_generation: generation('a') }), candidates: [ice('late-close')] });
    else pending.reject(new Error('closing owner response failed'));
    await flush(); const calls = f.calls.length;
    peer.emit('icecandidate', { candidate: null });
    for (const id of f.pollIds()) f.timers.get(id).callback();
    peer.connectionState = 'failed'; peer.emit('connectionstatechange'); await flush();
    assert.equal(f.calls.length, calls); assert.deepEqual(f.recovered, []);
    assert.ok(f.peers.every(peer => peer.added.length === 0)); f.controller.close();
  });
}

for (const outcome of ['success', 'failure']) test(`closing retained owner stops delayed answer ${outcome}`, async () => {
  const f = controllerFixture(), pending = deferred(), started = deferred();
  f.state.fetch = () => { started.resolve(); return pending.promise; };
  const connecting = f.controller.connect(f.display('a'), {}); await started.promise;
  f.state.active = false;
  if (outcome === 'success') pending.resolve(f.ack({ type: 'answer', display_generation: generation('a') }));
  else pending.reject(new Error('answer failed during close'));
  await connecting;
  assert.equal(f.peers.length, 2); assert.equal(f.focused.length, 0); assert.equal(f.pollIds().length, 0);
  assert.deepEqual(f.recovered, []); f.controller.close();
});

for (const owner of ['current', 'replaced']) {
  test(`queued local ICE send rejection recovers only the current owner: ${owner}`, async () => {
    const f = controllerFixture({ audio: false });
    const pending = deferred(), started = deferred();
    const unhandled = [];
    const onUnhandled = reason => unhandled.push(reason);
    process.on('unhandledRejection', onUnhandled);
    f.state.localIce = [ice('queued')];
    f.state.fetch = (body, ack) => {
      if (body.display_generation === generation('a') && body.type === 'candidate') {
        started.resolve();
        return pending.promise;
      }
      return ack(body);
    };
    try {
      const connecting = f.controller.connect(f.display('a'), {});
      await started.promise;
      if (owner === 'replaced') await f.controller.connect(f.display('b'), {});
      pending.reject(new Error('queued ice failed'));
      await connecting;
      await flush();
      await flush();
      if (owner === 'current') {
        assert.equal(f.recovered.length, 1);
        assert.equal(f.recovered[0][0], 'queued ice failed');
        assert.equal(f.recovered[0][1].failureKind, 'signaling');
      } else {
        assert.deepEqual(f.recovered, []);
      }
      assert.deepEqual(unhandled, []);
    } finally {
      process.off('unhandledRejection', onUnhandled);
      f.controller.close();
    }
  });
}

test('connect starts the audio peer before the first video frame', async () => {
  const f = controllerFixture();
  await f.controller.connect(f.display('a'), {});
  await flush();
  await flush();
  assert.equal(f.peers.length, 2);
  assert.equal(f.controller.isTrackReady(), false);
  f.controller.close();
});

test('viewer activity guard is captured for the owner at connect', async () => {
  const f = controllerFixture(); await f.controller.connect(f.display('a'), {});
  f.state.owner += 1; // Another owner is active; that does not authorize the existing peer.
  const calls = f.calls.length;
  f.peers[0].emit('icecandidate', { candidate: null }); await flush();
  assert.equal(f.calls.length, calls); assert.deepEqual(f.recovered, []); f.controller.close();
  f.state.active = false; const peers = f.peers.length;
  await f.controller.connect(f.display('b'), {});
  assert.equal(f.peers.length, peers);
});
