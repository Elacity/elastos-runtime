import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import { readBrowserViewerReloadDocument } from './lib/browser-journey-viewer-reload.mjs';
const source = readFileSync(new URL('../capsules/browser/browser/browser.js', import.meta.url), 'utf8');
const start = source.indexOf('async function readRemoteDisplayMetrics(');
const end = source.indexOf('\n}', start) + 2;
assert.ok(start >= 0 && end > start);
const readMetrics = source.slice(start, end);
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function viewer(query) {
  const elements = {
    '#browser-remote-display': { hidden: false, paused: false, readyState: 4, videoWidth: 1280, videoHeight: 720,
      webkitDecodedFrameCount: 42, getBoundingClientRect: () => ({ width: 1280, height: 720 }) },
    '#browser-url': { value: 'https://fixture.invalid/nav' }, '#browser-engine': { value: 'engine-one' }, '#browser-exit': { value: '' },
  };
  const state = vm.createContext({ URL, Number, performance: { timeOrigin: 1000 },
    location: { href: 'https://runtime.invalid/apps/browser/?browser_instance=instance-one' },
    document: { querySelector: name => elements[name] },
    window: { __elastosBrowserCurrentPageId: 'page-one', __elastosBrowserReadRemoteDisplayMetrics: query,
      __elastosBrowserRemoteDisplayMetrics: { latestVideoWebrtcStats: { video_bytes_received: 111 } } },
  });
  vm.runInContext(readBrowserViewerReloadDocument.toString(), state);
  return { state, elements, read: () => state.readBrowserViewerReloadDocument() };
}

test('reload document observation waits for fresh counters and ignores the cached HUD sample', async () => {
  const wait = deferred(), h = viewer(() => wait.promise); let settled = false;
  const pending = h.read().then(value => { settled = true; return value; });
  await new Promise(setImmediate); assert.equal(settled, false);
  wait.resolve({ latestVideoWebrtcStats: { video_bytes_received: 999 } });
  const value = await pending; assert.equal(value.video.video_bytes_received, 999);
  assert.equal(value.video.decoded_frames, 42); assert.equal(value.viewer.document_id, 1000);
  assert.equal(value.viewer.page_id, 'page-one');
});

for (const query of [undefined, async () => null]) test(`pending fresh diagnostics cannot borrow old counters: ${query ? 'retired peer' : 'module loading'}`, async () => {
  const value = await viewer(query).read(); assert.equal(Object.hasOwn(value.video, 'video_bytes_received'), false);
  assert.equal(value.video.decoded_frames, 42);
});

test('a failed fresh counter read remains a diagnostic failure', async () => {
  await assert.rejects(viewer(async () => { throw new Error('fresh query failed'); }).read(), /fresh query failed/);
});

for (const change of ['close', 'owner', 'none']) test(`actual Browser diagnostic bridge retains captured ownership after ${change}`, async () => {
  const wait = deferred(), first = {}, second = {}, updates = [];
  const state = vm.createContext({ owner: first, active: true, window: {}, lastPageStatus: {},
    currentRuntimePageOwner: () => state.owner,
    runtimeViewerOwnerActive: owner => state.active && owner === state.owner,
    remoteDisplay: { refreshMetrics: () => wait.promise },
    updateMetricsNode: () => { updates.push(true); state.window.__elastosBrowserRemoteDisplayMetrics = { fresh: true }; },
  });
  vm.runInContext(readMetrics, state);
  const pending = state.readRemoteDisplayMetrics();
  if (change === 'close') state.active = false;
  if (change === 'owner') state.owner = second;
  wait.resolve({}); const result = await pending;
  assert.equal(updates.length, change === 'none' ? 1 : 0);
  assert.equal(result?.fresh || false, change === 'none');
});


test('layout and frame state are sampled after the fresh counters finish', async () => {
  const wait = deferred(), h = viewer(() => wait.promise);
  const pending = h.read();
  h.elements['#browser-remote-display'] = { hidden: false, paused: false, readyState: 4,
    videoWidth: 1280, videoHeight: 720, webkitDecodedFrameCount: 99,
    getBoundingClientRect: () => ({ width: 0, height: 0 }) };
  wait.resolve({ latestVideoWebrtcStats: { video_bytes_received: 999 } });
  const value = await pending;
  assert.equal(value.video.client_width, 0); assert.equal(value.video.client_height, 0);
  assert.equal(value.video.decoded_frames, 99); assert.equal(value.video.video_bytes_received, 999);
});
