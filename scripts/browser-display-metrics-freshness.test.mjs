import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('../capsules/browser/browser/browser-remote-display.js', import.meta.url), 'utf8');
const statusSource = readFileSync(new URL('../capsules/browser/browser/browser-status.js', import.meta.url), 'utf8');
function deferred() {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}
const statsReport = (kind, bytes, frames = 0) => new Map([['inbound', {
  id: 'inbound', type: 'inbound-rtp', kind, bytesReceived: bytes,
  ...(kind === 'video' ? { framesDecoded: frames } : {}),
}]]);

// Run the actual controller, nested timer callbacks, and stats projection.
// Only browser objects and scheduling are controlled by this fixture.
function fixture({ debugMetrics = true, audio = true } = {}) {
  class Events {
    listeners = new Map();
    addEventListener(type, callback) {
      this.listeners.set(type, [...(this.listeners.get(type) || []), callback]);
    }
  }
  class Media extends Events {
    srcObject = null; muted = true; hidden = true; currentTime = 0;
    videoWidth = 0; videoHeight = 0;
    play() { return Promise.resolve(); }
    pause() {}
  }
  class Stream {
    tracks = [];
    getTracks() { return this.tracks; }
    getAudioTracks() { return []; }
    addTrack(track) { this.tracks.push(track); }
  }
  const peers = [], timers = new Map(), updates = [], recoveries = [];
  const state = { active: true, owner: 1, fetch: null };
  class Peer extends Events {
    connectionState = 'new'; iceConnectionState = 'new'; signalingState = 'stable';
    reads = 0; bytes = 100; frames = 10; response = null;
    constructor() { super(); this.kind = peers.length % (audio ? 2 : 1) ? 'audio' : 'video'; peers.push(this); }
    addTransceiver() {}
    async setRemoteDescription() {}
    async setLocalDescription(value) { this.localDescription = value; }
    async createAnswer() { return { type: 'answer', sdp: 'v=0\r\n' }; }
    async addIceCandidate() {}
    getStats() {
      this.reads += 1;
      return this.response ? this.response() : Promise.resolve(statsReport(this.kind, this.bytes, this.frames));
    }
    close() { this.connectionState = 'closed'; this.signalingState = 'closed'; }
  }
  let timerId = 0;
  const context = vm.createContext({ Audio: Media, MediaStream: Stream, RTCPeerConnection: Peer, Blob,
    console: { info() {} },
    window: {
      setTimeout(callback, delay) { timers.set(++timerId, { callback, delay }); return timerId; },
      clearTimeout(id) { timers.delete(id); },
    },
    iceCandidateType: () => 'relay', normalizeDisplayIceServers: value => value || [],
    normalizeEngineCandidate: value => value, normalizeIceCandidateForRuntime: value => value,
    sdpHasOnlyRelayCandidates: () => true, stripTrickleCandidatesFromSdp: value => value,
    validateRuntimeLaunchTurn: async () => [],
  });
  vm.runInContext(statusSource.slice(statusSource.indexOf('export async function collectWebrtcStats'), statusSource.indexOf('export function browserMetricsText')).replace('export async function', 'async function'), context);
  vm.runInContext(source.slice(source.indexOf('const WEBRTC_CONNECT_TIMEOUT_MS')).replace('export function createBrowserRemoteDisplay', 'function createBrowserRemoteDisplay'), context);
  const ack = body => ({ schema: 'elastos.browser.webrtc-signal-ack/v1', type: body.type, accepted: true, candidates: [] });
  const controller = context.createBrowserRemoteDisplay({ debugMetrics,
    fetchJson: async (url, options) => state.fetch ? state.fetch(options.body) : ack(options.body),
    friendlyOpenError: error => error.message, getCurrentDisplayMode: () => 'webrtc_remote_display',
    getLastPageStatus: () => ({}),
    captureViewerOwnerGuard: () => { const owner = state.owner; return () => state.active && state.owner === owner; },
    handleRemoteInputChannelMessage() {}, onRecoveryRequired: async (...args) => recoveries.push(args),
    remoteVideo: new Media(), renderEmpty: {}, renderPanel: { focus() {} },
    resetPageStatus() {}, setActiveBrowserPage() {}, setDisplayInput() {}, showStatus() {},
    updateMetrics: value => updates.push({ status: value, metrics: controller.metricsState() }),
  });
  const display = { schema: 'elastos.browser.display-session/v1', mode: 'webrtc_remote_display', offerer: 'engine', audio,
    signaling_url: '/api/apps/browser/pages/page-one/webrtc',
    initial_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'v=0\r\n' },
    audio_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'v=0\r\n' },
  };
  const pollIds = () => [...timers].filter(([, timer]) => timer.delay === 1000).map(([id]) => id);
  const firePoll = () => {
    const [id] = pollIds(); assert.ok(id, 'one-second poll exists');
    const { callback } = timers.get(id); timers.delete(id); return callback();
  };
  return { controller, connect: () => controller.connect(display, {}), peers, timers, updates, recoveries, state, ack, pollIds, firePoll };
}

test('on-demand refresh reads fresh video/audio counters without waiting for or changing timers', async () => {
  const f = fixture({ debugMetrics: false });
  assert.equal(await f.controller.refreshMetrics(), null);
  await f.connect(); const timers = [...f.timers.keys()];
  const first = await f.controller.refreshMetrics();
  assert.equal(first.latestWebrtcStats.video_bytes_received, 100);
  assert.equal(first.latestWebrtcStats.audio_bytes_received, 100);
  f.peers[0].bytes = 250; f.peers[0].frames = 19; f.peers[1].bytes = 180;
  assert.equal(f.controller.metricsState().latestWebrtcStats.video_bytes_received, 100);
  const fresh = await f.controller.refreshMetrics();
  assert.equal(fresh.latestWebrtcStats.video_bytes_received, 250);
  assert.equal(fresh.latestVideoWebrtcStats.video_frames_decoded, 19);
  assert.equal(fresh.latestAudioWebrtcStats.audio_bytes_received, 180);
  assert.deepEqual(f.peers.map(peer => peer.reads), [2, 2]);
  assert.deepEqual([...f.timers.keys()], timers); assert.deepEqual(f.recoveries, []);
  f.controller.close();
});

test('steady polling uses fresh queries and keeps the one-second cadence after a query failure', async () => {
  const f = fixture(); await f.connect(); await f.firePoll();
  assert.equal(f.controller.metricsState().latestWebrtcStats.video_bytes_received, 100);
  assert.equal(f.updates.length, 1); assert.equal(f.pollIds().length, 1);
  f.peers[0].response = async () => { throw new Error('stats unavailable'); };
  await f.firePoll();
  assert.equal(f.updates.length, 1); assert.equal(f.pollIds().length, 1);
  f.peers[0].response = null; f.peers[0].bytes = 300;
  await f.firePoll();
  assert.equal(f.controller.metricsState().latestWebrtcStats.video_bytes_received, 300);
  assert.equal(f.updates.length, 2); assert.equal(f.pollIds().length, 1);
  assert.deepEqual(f.recoveries, []); f.controller.close(); assert.equal(f.pollIds().length, 0);
});

for (const channel of ['video', 'audio']) for (const action of ['replacement', 'close', 'pending-close']) for (const outcome of ['success', 'failure']) {
  test(`late ${channel} stats ${outcome} after ${action} returns null and preserves current metrics`, async () => {
    const f = fixture(); await f.connect(); await f.controller.refreshMetrics();
    const peer = f.peers[channel === 'video' ? 0 : 1], pending = deferred();
    peer.response = () => pending.promise;
    const refreshing = f.controller.refreshMetrics();
    if (action === 'replacement') { f.state.owner += 1; await f.connect(); await f.controller.refreshMetrics(); }
    else if (action === 'close') f.controller.close();
    else f.state.active = false;
    const current = f.controller.metricsState(), polls = f.pollIds();
    if (outcome === 'success') pending.resolve(statsReport(channel, 9999));
    else pending.reject(new Error('retired query failed'));
    assert.equal(await refreshing, null);
    assert.deepEqual(f.controller.metricsState(), current); assert.deepEqual(f.pollIds(), polls);
    assert.deepEqual(f.recoveries, []);
    if (action !== 'replacement') {
      const reads = f.peers.map(peer => peer.reads);
      assert.equal(await f.controller.refreshMetrics(), null);
      assert.deepEqual(f.peers.map(peer => peer.reads), reads);
    }
    f.controller.close();
  });
}

for (const action of ['replacement', 'close', 'pending-close']) for (const outcome of ['success', 'failure']) {
  test(`old polling ${outcome} after ${action} cannot publish or replace the fresh timer`, async () => {
    const f = fixture(); await f.connect(); const pending = deferred();
    f.peers[1].response = () => pending.promise;
    const polling = f.firePoll();
    if (action === 'replacement') { f.state.owner += 1; await f.connect(); await f.firePoll(); }
    else if (action === 'close') f.controller.close();
    else f.state.active = false;
    const current = f.controller.metricsState(), polls = f.pollIds(), updates = f.updates.length;
    if (outcome === 'success') pending.resolve(statsReport('audio', 9999));
    else pending.reject(new Error('old polling failed'));
    await polling;
    assert.deepEqual(f.controller.metricsState(), current); assert.equal(f.updates.length, updates);
    assert.deepEqual(f.pollIds(), polls); assert.deepEqual(f.recoveries, []);
    f.controller.close(); assert.equal(f.pollIds().length, 0, 'close still owns the current timer');
  });
}

for (const order of ['poll-first', 'on-demand-first']) test(`simultaneous ${order} queries share fresh counters without null or cached fallback`, async () => {
  const f = fixture(); await f.connect(); await f.controller.refreshMetrics();
  const video = deferred(), audio = deferred();
  f.peers[0].response = () => video.promise; f.peers[1].response = () => audio.promise;
  let polling, refreshing;
  if (order === 'poll-first') { polling = f.firePoll(); refreshing = f.controller.refreshMetrics(); }
  else { refreshing = f.controller.refreshMetrics(); polling = f.firePoll(); }
  const anotherReader = f.controller.refreshMetrics();
  video.resolve(statsReport('video', 500, 57)); audio.resolve(statsReport('audio', 700));
  const [fresh, sameQuery] = await Promise.all([refreshing, anotherReader, polling]);
  assert.ok(fresh, 'the awaited diagnostic query stays valid during polling');
  assert.equal(fresh, sameQuery);
  assert.equal(fresh.latestWebrtcStats.video_bytes_received, 500);
  assert.equal(fresh.latestWebrtcStats.audio_bytes_received, 700);
  assert.deepEqual(f.peers.map(peer => peer.reads), [2, 2], 'one baseline plus one shared query');
  assert.equal(f.updates.length, 1); assert.equal(f.pollIds().length, 1);
  assert.equal(f.updates[0].metrics.latestWebrtcStats.video_bytes_received, 500);
  f.peers[0].response = null; f.peers[0].bytes = 900; f.peers[1].response = null;
  assert.equal((await f.controller.refreshMetrics()).latestWebrtcStats.video_bytes_received, 900);
  assert.deepEqual(f.peers.map(peer => peer.reads), [3, 3], 'a completed query is not a reusable cache');
  f.controller.close();
});

for (const outcome of ['success', 'failure']) test(`old query ${outcome} cannot clear a replacement query that is still pending`, async () => {
  const f = fixture(); await f.connect(); const old = deferred();
  f.peers[0].response = () => old.promise;
  const previous = f.controller.refreshMetrics();
  f.state.owner += 1; await f.connect(); const pending = deferred();
  f.peers[2].response = () => pending.promise;
  const current = f.controller.refreshMetrics();
  if (outcome === 'success') old.resolve(statsReport('video', 1));
  else old.reject(new Error('old query failed'));
  assert.equal(await previous, null);
  const joined = f.controller.refreshMetrics();
  pending.resolve(statsReport('video', 800));
  const [fresh, sameQuery] = await Promise.all([current, joined]);
  assert.ok(fresh); assert.equal(fresh, sameQuery);
  assert.equal(fresh.latestWebrtcStats.video_bytes_received, 800);
  assert.deepEqual(f.peers.slice(2).map(peer => peer.reads), [1, 1]);
  assert.deepEqual(f.recoveries, []); f.controller.close();
});

test('audio peer appearing during a video-only query invalidates the captured pair', async () => {
  const f = fixture(), answer = deferred(), started = deferred(), stats = deferred();
  f.state.fetch = body => {
    if (!body.channel) { started.resolve(); return answer.promise; }
    return f.ack(body);
  };
  const connecting = f.connect(); await started.promise;
  assert.equal(f.peers.length, 1); f.peers[0].response = () => stats.promise;
  const refreshing = f.controller.refreshMetrics();
  answer.resolve(f.ack({ type: 'answer' })); await connecting;
  assert.equal(f.peers.length, 2);
  stats.resolve(statsReport('video', 100)); assert.equal(await refreshing, null);
  assert.equal(f.controller.metricsState().latestWebrtcStats, null);
  f.peers[0].response = null;
  assert.equal((await f.controller.refreshMetrics()).latestWebrtcStats.audio_bytes_received, 100);
  f.controller.close();
});

test('video-only display returns fresh metrics without requiring an audio peer', async () => {
  const f = fixture({ audio: false }); await f.connect();
  const fresh = await f.controller.refreshMetrics();
  assert.equal(fresh.latestWebrtcStats.video_bytes_received, 100);
  assert.equal(fresh.latestAudioWebrtcStats, null);
  assert.equal(f.peers.length, 1); f.controller.close();
});
