import assert from 'node:assert/strict';
import test from 'node:test';
import net from 'node:net';
import { once } from 'node:events';
import { SelkiesPage, MinimalWebSocketClient } from './browser-selkies-control-service.mjs';

const id = 'a'.repeat(32), otherId = 'b'.repeat(32);
const videoSdp = 'v=0\r\nm=video 9 UDP/TLS/RTP/SAVPF 96\r\n';
const audioSdp = 'v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\n';
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function fixture() {
  const calls = [], closed = [];
  const config = { selkiesWsUrl: new URL('ws://127.0.0.1:1'),
    displaySurface: { stream: { width: 1280, height: 720 } }, iceServers: [] };
  const page = new SelkiesPage(config, { page_id: 'page:test', stream_id: 'stream:test', adapter: 'test', engine: 'chromium' }, value => closed.push(value));
  page.browserPage = { url: 'https://retained.invalid/', title: 'Retained page' };
  page.wallet = { accounts: [], default_chain_namespace: 'eip155' };
  page.displaySession = page.supervisorResult(videoSdp, page.browserPage, page.wallet, audioSdp).display_session;
  page.displayAvailable = true;
  const socket = channel => {
    const value = { close() {
      calls.push(['close', channel]);
      if (channel === 'video' && page.ws === value) page.markVideoClosed();
      if (channel === 'audio' && page.audioWs === value) page.markAudioClosed();
    }, sendText(text) { calls.push(['send', channel, text]); } };
    return value;
  };
  page.createWebSocket = () => socket('video'); page.createAudioWebSocket = () => socket('audio');
  page.resetSignaling(); page.resetAudioSignaling();
  page.signalingEnvelope = page.audioSignalingEnvelope = 'raw_json';
  page.establishedVideoProtocol = page.establishedAudioProtocol = 'raw_json';
  page.openLegacySelkiesSession = async () => { calls.push(['open', 'video']); page.signalingEnvelope = 'raw_json'; return { sdp: { sdp: videoSdp } }; };
  page.openLegacySelkiesAudioSession = async () => { calls.push(['open', 'audio']); page.audioSignalingEnvelope = 'raw_json'; return { sdp: { sdp: audioSdp } }; };
  const generation = page.displayGeneration;
  const request = { schema: 'elastos.browser.display-attach-request/v1', type: 'display_attach', request_id: id, display_generation: generation };
  return { page, calls, closed, generation, request };
}

test('paired attachment preserves the page and returns only fresh display offers', async () => {
  const f = fixture(), browserPage = f.page.browserPage, oldVideo = f.page.ws, oldAudio = f.page.audioWs;
  f.page.remoteCandidateHistory.push({ candidate: 'old-video' });
  f.page.audioRemoteCandidateHistory.push({ candidate: 'old-audio' });
  const result = await f.page.signal(f.request);
  assert.deepEqual(f.calls, [['close', 'video'], ['close', 'audio'], ['open', 'video'], ['open', 'audio']]);
  assert.notEqual(result.display_generation, f.generation);
  assert.equal(result.previous_display_generation, f.generation);
  assert.equal(result.page_id, 'page:test'); assert.equal(result.request_id, id);
  assert.deepEqual(result.initial_offer.candidates, []); assert.deepEqual(result.audio_offer.candidates, []);
  assert.deepEqual(Object.keys(result).sort(), ['schema', 'page_id', 'request_id', 'previous_display_generation', 'display_generation', 'initial_offer', 'audio_offer'].sort());
  assert.equal(f.page.browserPage, browserPage); assert.equal(f.page.closed, false); assert.deepEqual(f.closed, []);
  oldVideo.close(); oldAudio.close();
  assert.equal(f.page.displayAvailable, true); assert.equal(f.page.videoClosed, false); assert.equal(f.page.audioClosed, false);
});

test('same request joins in flight and replays after success before stale-generation validation', async () => {
  const f = fixture(), wait = deferred(); f.page.openLegacySelkiesSession = async () => wait.promise;
  const pending = f.page.signal(f.request); assert.equal(f.page.signal({ ...f.request }), pending);
  assert.throws(() => f.page.signal({ ...f.request, request_id: otherId }), { code: 'display_attach_busy' });
  wait.resolve({ sdp: { sdp: videoSdp } }); const result = await pending;
  const calls = f.calls.length; assert.equal(await f.page.signal({ ...f.request }), result); assert.equal(f.calls.length, calls);
  assert.throws(() => f.page.signal({ ...f.request, display_generation: result.display_generation }), { code: 'display_generation_mismatch' });
  assert.throws(() => f.page.signal({ ...f.request, request_id: otherId }), { code: 'display_generation_mismatch' });
});

test('initial legacy signals work; attachment requires current generation and echoes it', async () => {
  const f = fixture(), answer = { schema: 'elastos.browser.webrtc-answer/v1', type: 'answer', sdp: 'answer' };
  assert.equal(f.page.signal(answer).accepted, true);
  const result = await f.page.signal(f.request), before = f.calls.length;
  for (const generation of [undefined, f.generation]) {
    assert.throws(() => f.page.signal({ ...answer, display_generation: generation }), { code: 'display_generation_mismatch' });
    assert.throws(() => f.page.signal({ schema: 'elastos.browser.webrtc-candidate/v1', candidate: {}, display_generation: generation }, 'audio'), { code: 'display_generation_mismatch' });
  }
  assert.equal(f.calls.length, before);
  for (const channel of ['video', 'audio']) {
    const ack = f.page.signal({ ...answer, display_generation: result.display_generation }, channel);
    assert.equal(ack.display_generation, result.display_generation);
  }
});

test('explicit close wins over late video negotiation', async () => {
  const f = fixture(), wait = deferred(); f.page.openLegacySelkiesSession = async () => wait.promise;
  const pending = f.page.signal(f.request); await Promise.resolve(); f.page.close();
  wait.resolve({ sdp: { sdp: videoSdp } }); await assert.rejects(pending, { code: 'display_attach_failed' });
  assert.equal(f.page.displayGeneration, f.generation); assert.equal(f.page.displayAvailable, false);
  assert.equal(f.closed.length, 1); assert.equal(f.calls.some(row => row[0] === 'open' && row[1] === 'audio'), false);
  f.page.close(); assert.equal(f.closed.length, 1);
});

test('partial attachment failure retains cleanup ownership and makes failure replay side-effect free', async () => {
  const f = fixture(); f.page.openLegacySelkiesAudioSession = async () => { throw new Error('private backend detail'); };
  await assert.rejects(f.page.signal(f.request), { code: 'display_attach_failed' });
  assert.equal(f.page.closed, false); assert.equal(f.page.displayAvailable, false); assert.deepEqual(f.closed, []);
  assert.equal(f.page.displayGeneration, f.generation); const before = f.calls.length;
  await assert.rejects(f.page.signal(f.request), { code: 'display_attach_failed' }); assert.equal(f.calls.length, before);
  f.page.close(); assert.equal(f.closed.length, 1);
});

test('paired retirement rejects old pending waiters instead of clearing their deadlines', async () => {
  const f = fixture(); f.page.config.signalTimeoutMs = 1000;
  const video = assert.rejects(f.page.waitFor(() => false, 'old video'), /closed/);
  const audio = assert.rejects(f.page.waitForAudio(() => false, 'old audio'), /closed/);
  await f.page.signal(f.request); await Promise.all([video, audio]);
  assert.equal(f.page.waiters.length, 0); assert.equal(f.page.audioWaiters.length, 0);
});

test('video signaling loss preserves page ownership for explicit close', () => {
  const f = fixture(); f.page.ws.close();
  assert.equal(f.page.closed, false); assert.equal(f.page.videoClosed, true); assert.equal(f.page.displayAvailable, false); assert.deepEqual(f.closed, []);
  f.page.close(); assert.equal(f.closed.length, 1);
});

test('attachment deadline preserves owner and prevents late negotiation publication', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture(), wait = deferred(); f.page.openLegacySelkiesSession = async () => wait.promise;
  const pending = f.page.signal(f.request); await Promise.resolve();
  const failed = assert.rejects(pending, { code: 'display_attach_failed' });
  t.mock.timers.tick(4000); await failed;
  wait.resolve({ sdp: { sdp: videoSdp } }); await Promise.resolve(); await Promise.resolve();
  assert.equal(f.page.displayGeneration, f.generation); assert.equal(f.page.displayAvailable, false);
  assert.equal(f.page.closed, false); assert.deepEqual(f.closed, []);
  assert.equal(f.calls.some(row => row[0] === 'open' && row[1] === 'audio'), false);
});

for (const invalid of [{ request_id: 'bad' }, { display_generation: 'bad' }, { type: 'answer' }, { sdp: '' }, { channel: 'audio' }, { extra: true }]) {
  test(`invalid attachment ${Object.keys(invalid)[0]} acquires no display effects`, () => {
    const f = fixture(); assert.throws(() => f.page.signal({ ...f.request, ...invalid }), { code: 'invalid_request' });
    assert.deepEqual(f.calls, []); assert.equal(f.page.displayGeneration, f.generation);
  });
}
test('attachment forbids a channel and legacy pages report unsupported before reset', () => {
  const f = fixture(); assert.throws(() => f.page.signal(f.request, 'audio'), { code: 'invalid_request' });
  f.page.displaySession = null;
  assert.throws(() => f.page.signal(f.request), { code: 'display_attach_unsupported' });
  assert.deepEqual(f.calls, []);
});

test('timed-out negotiation cannot send HELLO or register waiters on the next attempt', async t => {
  t.mock.timers.enable({ apis: ['setTimeout'] });
  const f = fixture(), first = deferred(), second = deferred(), sends = [], waits = [];
  delete f.page.openLegacySelkiesSession; delete f.page.openLegacySelkiesAudioSession;
  let sockets = 0;
  f.page.createWebSocket = () => {
    const index = ++sockets;
    return { connect: async () => (index === 1 ? first : second).promise,
      sendText: value => sends.push([index, value.split(' ')[0]]), close() {} };
  };
  f.page.createAudioWebSocket = () => ({ connect: async () => {}, sendText: () => {}, close() {} });
  f.page.waitFor = async (_predicate, label) => { waits.push(label); return label.includes('HELLO') ? { kind: 'hello' } : { sdp: { sdp: videoSdp } }; };
  f.page.waitForAudio = async (_predicate, label) => label.includes('HELLO') ? { kind: 'hello' } : { sdp: { sdp: audioSdp } };
  f.page.openCurrentSelkiesSession = async () => { throw new Error('Lost legacy protocol'); };
  const a = f.page.signal(f.request); await Promise.resolve();
  const failed = assert.rejects(a, { code: 'display_attach_failed' }); t.mock.timers.tick(4000); await failed;
  const b = f.page.signal({ ...f.request, request_id: otherId }); await Promise.resolve();
  first.resolve(); await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
  assert.deepEqual(sends, []); assert.deepEqual(waits, []);
  second.resolve(); await b;
  assert.deepEqual(sends, [[2, 'HELLO']]); assert.equal(waits.length, 2);
  assert.equal(f.page.closed, false); assert.equal(f.page.displayAvailable, true);
});

test('actual socket close cancels a stalled HTTP handshake and drains the connection', async () => {
  const accepted = deferred(), sockets = new Set();
  const server = net.createServer(socket => { sockets.add(socket); socket.once('close', () => sockets.delete(socket)); socket.once('data', () => accepted.resolve(socket)); });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  const ws = new MinimalWebSocketClient(new URL(`ws://127.0.0.1:${server.address().port}`));
  try {
    const connecting = ws.connect(1000), rejected = assert.rejects(connecting, /closed|canceled/);
    const socket = await accepted.promise, closed = once(socket, 'close');
    ws.close(); await rejected; await closed;
    assert.equal(ws.socket.destroyed, true); assert.equal(ws.closed, true); assert.equal(sockets.size, 0);
  } finally { ws.close(); for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); }
});
