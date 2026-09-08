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

async function legacyBroker(t, { releaseMs = 120, rejectionReason = 'invalid peer uid', acknowledgeRejected = false } = {}) {
  const peers = new Map(), sockets = new Set(), timers = new Set(), rejected = [], accepted = [];
  const frame = (opcode, payload) => {
    payload = Buffer.isBuffer(payload) ? payload : Buffer.from(payload);
    assert.ok(payload.length < 126);
    return Buffer.concat([Buffer.from([0x80 | opcode, payload.length]), payload]);
  };
  const server = net.createServer(socket => {
    sockets.add(socket); let buffer = Buffer.alloc(0), upgraded = false, uid = null;
    socket.on('error', () => {});
    socket.on('close', () => {
      sockets.delete(socket);
      if (uid && peers.get(uid) === socket) {
        const timer = setTimeout(() => { timers.delete(timer); if (peers.get(uid) === socket) peers.delete(uid); }, typeof releaseMs === 'number' ? releaseMs : releaseMs[uid]);
        timers.add(timer);
      }
    });
    socket.on('data', chunk => {
      buffer = Buffer.concat([buffer, chunk]);
      if (!upgraded) {
        const end = buffer.indexOf('\r\n\r\n'); if (end < 0) return;
        buffer = buffer.subarray(end + 4); upgraded = true;
        socket.write('HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n\r\n');
      }
      while (buffer.length >= 2) {
        const opcode = buffer[0] & 15, masked = !!(buffer[1] & 128);
        let length = buffer[1] & 127, offset = 2;
        if (length === 126) { if (buffer.length < 4) return; length = buffer.readUInt16BE(2); offset = 4; }
        assert.notEqual(length, 127); assert.equal(masked, true);
        if (buffer.length < offset + 4 + length) return;
        const mask = buffer.subarray(offset, offset + 4), payload = Buffer.from(buffer.subarray(offset + 4, offset + 4 + length));
        buffer = buffer.subarray(offset + 4 + length);
        for (let i = 0; i < payload.length; i++) payload[i] ^= mask[i % 4];
        if (opcode === 8) { socket.end(frame(8, Buffer.alloc(0))); return; }
        if (opcode !== 1) continue;
        const words = payload.toString().split(' '); assert.equal(words[0], 'HELLO');
        const requested = words[1];
        if (peers.has(requested)) {
          rejected.push(requested);
          assert.notEqual(peers.get(requested), socket);
          const code = Buffer.alloc(2); code.writeUInt16BE(1002);
          if (acknowledgeRejected) socket.write(frame(1, 'HELLO'));
          socket.end(frame(8, Buffer.concat([code, Buffer.from(rejectionReason)]))); return;
        }
        uid = requested; peers.set(uid, socket); accepted.push(uid);
        socket.write(Buffer.concat([frame(1, 'HELLO'), frame(1, JSON.stringify({ sdp: { type: 'offer', sdp: uid === '1' ? videoSdp : audioSdp } }))]));
      }
    });
  });
  server.listen(0, '127.0.0.1'); await once(server, 'listening');
  t.after(async () => { for (const socket of sockets) socket.destroy(); await new Promise(resolve => server.close(resolve)); for (const timer of timers) clearTimeout(timer); });
  return { url: new URL(`ws://127.0.0.1:${server.address().port}`), rejected, accepted };
}

async function actualLegacyPage(t, broker) {
  const f = fixture();
  delete f.page.createWebSocket; delete f.page.createAudioWebSocket;
  delete f.page.openLegacySelkiesSession; delete f.page.openLegacySelkiesAudioSession;
  Object.assign(f.page.config, { selkiesWsUrl: broker.url, connectTimeoutMs: 1000, signalTimeoutMs: 1000 });
  f.page.resetSignaling(); f.page.resetAudioSignaling();
  await f.page.openLegacySelkiesSession({ scale: 1 });
  await f.page.openLegacySelkiesAudioSession({ scale: 1 });
  t.after(() => f.page.close());
  return f;
}

test('actual legacy broker keeps retired viewer IDs until producer teardown; attachment retries only UID rejection', async t => {
  const broker = await legacyBroker(t, { releaseMs: { '1': 120, '3': 320 } }), f = await actualLegacyPage(t, broker);
  const result = await f.page.signal(f.request);
  assert.ok(broker.rejected.includes('1'));
  assert.ok(broker.rejected.includes('3'));
  assert.deepEqual(broker.accepted, ['1', '3', '1', '3']);
  assert.equal(f.page.closed, false); assert.equal(f.page.displayAvailable, true);
  assert.equal(result.initial_offer.type, 'offer'); assert.equal(result.audio_offer.type, 'offer');
  assert.notEqual(result.display_generation, f.generation);
  const next = await f.page.signal({ ...f.request, request_id: otherId, display_generation: result.display_generation });
  assert.notEqual(next.display_generation, result.display_generation);
  assert.deepEqual(broker.accepted, ['1', '3', '1', '3', '1', '3']);
});

test('UID error after HELLO acknowledgment is terminal even when its code and reason match', async t => {
  const broker = await legacyBroker(t, { acknowledgeRejected: true }), f = await actualLegacyPage(t, broker);
  await assert.rejects(f.page.signal(f.request), { code: 'display_attach_failed' });
  assert.deepEqual(broker.rejected, ['1']);
});

test('actual broker protocol rejection other than the exact UID release response remains terminal', async t => {
  const broker = await legacyBroker(t, { rejectionReason: 'invalid protocol' }), f = await actualLegacyPage(t, broker);
  await assert.rejects(f.page.signal(f.request), { code: 'display_attach_failed' });
  assert.deepEqual(broker.rejected, ['1']); assert.equal(f.page.closed, false);
});

for (const cancel of ['deadline', 'close']) {
  test(`legacy UID release retry obeys the original ${cancel} and cannot open a later socket`, async t => {
    t.mock.timers.enable({ apis: ['setTimeout'] });
    const f = fixture(); let attempts = 0;
    f.page.openLegacySelkiesSession = async () => {
      attempts++; f.page.ws.closeCode = 1002; f.page.ws.closeReason = 'invalid peer uid'; throw new Error('closed during HELLO');
    };
    const pending = f.page.signal(f.request), failed = assert.rejects(pending, { code: 'display_attach_failed' });
    for (let i = 0; i < 6; i++) await Promise.resolve();
    assert.equal(attempts, 1);
    if (cancel === 'close') f.page.close();
    t.mock.timers.tick(cancel === 'close' ? 50 : 4000);
    await failed; for (let i = 0; i < 6; i++) await Promise.resolve();
    assert.equal(attempts, 1); assert.equal(f.page.displayGeneration, f.generation); assert.equal(f.page.displayAvailable, false);
    if (cancel === 'deadline') { await assert.rejects(f.page.signal(f.request), { code: 'display_attach_failed' }); assert.equal(attempts, 1); }
  });
}

test('overdue retry cannot acquire a socket when the event loop resumes after the attachment deadline', async () => {
  const f = fixture(); let attempts = 0;
  f.page.openLegacySelkiesSession = async () => {
    attempts++; f.page.ws.closeCode = 1002; f.page.ws.closeReason = 'invalid peer uid'; throw new Error('closed during HELLO');
  };
  const pending = f.page.signal(f.request), failed = assert.rejects(pending, { code: 'display_attach_failed' });
  for (let i = 0; i < 6; i++) await Promise.resolve();
  assert.equal(attempts, 1);
  // Both the 50ms retry and 4s cancellation timer become overdue. The retry's
  // timer is due first, so elapsed monotonic time must fence its continuation.
  Atomics.wait(new Int32Array(new SharedArrayBuffer(4)), 0, 0, 4050);
  await failed; assert.equal(attempts, 1); assert.equal(f.page.displayGeneration, f.generation);
});

for (const method of ['openLegacySelkiesSession', 'openLegacySelkiesAudioSession', 'openCurrentSelkiesSession', 'openCurrentSelkiesAudioSession']) {
  test(`${method} checks elapsed attachment time before HELLO after connection`, async () => {
    const f = fixture(), connected = deferred(), sends = [];
    delete f.page.openLegacySelkiesSession; delete f.page.openLegacySelkiesAudioSession;
    const socket = { connect: () => connected.promise, close() {}, sendText: value => sends.push(value) };
    if (method.includes('Audio')) f.page.audioWs = socket; else f.page.ws = socket;
    f.page.displayAttachment = { pending: true, deadline: performance.now() + 4000 };
    const pending = f.page[method]({ scale: 1 }), failed = assert.rejects(pending, /expired/);
    f.page.displayAttachment.deadline = performance.now() - 1; connected.resolve();
    await failed; assert.deepEqual(sends, []); assert.equal(f.page.waiters.length, 0); assert.equal(f.page.audioWaiters.length, 0);
  });
}

test('offers completed after elapsed attachment deadline cannot publish a generation', async () => {
  const f = fixture(), offer = deferred(), entered = deferred();
  f.page.openLegacySelkiesAudioSession = async () => { entered.resolve(); return offer.promise; };
  const pending = f.page.signal(f.request), failed = assert.rejects(pending, { code: 'display_attach_failed' });
  await entered.promise; f.page.displayAttachment.deadline = performance.now() - 1;
  offer.resolve({ sdp: { sdp: audioSdp } }); await failed;
  assert.equal(f.page.displayGeneration, f.generation); assert.equal(f.page.displayAvailable, false);
});
