import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
import crypto from 'node:crypto';
import { runtimePageOwner, sameRuntimePageOwner } from '../capsules/browser/browser/browser-page-cleanup.js';
const source = readFileSync(new URL('../capsules/browser/browser/browser.js', import.meta.url), 'utf8');
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, name);
  return source.slice(start, start + end + 2);
}
function deferred() { let resolve, reject; const promise = new Promise((a, b) => { resolve = a; reject = b; }); return { promise, resolve, reject }; }
function summary() {
  return { engine_adapter: { display_attach_supported: true }, sessions: { schema: 'elastos.browser.session-capacity/v1', status: 'configured', capacity_available: true, fresh_start_allowed: true, recoverable_page: {
    schema: 'elastos.browser.recoverable-page/v1', state: 'active', page_id: 'page-one',
    cleanup: { schema: 'elastos.browser.cleanup-handle/v1', id: 'cleanup-one' },
    service_selection: { schema: 'elastos.browser.service-selection/v1', engine_id: 'selected-engine', exit_id: 'selected-exit' },
    engine_page: { schema: 'elastos.browser.engine.page/v1', page_id: 'page-one', actual_url: 'https://old.invalid/',
      adapter: 'actual-engine', display_session: { mode: 'webrtc_remote_display', width: 1280, height: 720, source: 'runtime-recovery', display_generation: 'display:' + '1'.repeat(32), runtime_turn: { generation: 'owned-generation' } } },
  } } };
}
function harness() {
  const calls = [], failures = [], requests = [];
  const state = vm.createContext({ crypto, currentPage: null, currentPageGeneration: 0, nextPageGeneration: 1,
    restoredViewerOwner: null, unloadCleanupStarted: false, relaunchRequested: false,
    homeWindowCloseInFlight: false, homeWindowTerminalCloseConfirmed: false,
    pendingHomeWindowCloseDelivery: null, runtimePageCleanup: { status: () => null },
    runtimeOwnershipTerminallyAbsent: true, selectedBrowserEngineId: '', currentBrowserEngineId: '',
    selectedRemoteExitId: '', currentRemoteExitId: '', currentDisplayMode: '', currentView: null, lastPageStatus: null,
    runtimePageOwner, sameRuntimePageOwner,
    publishRuntimePageForHost: page => calls.push(['publish', page.page_id]),
    syncEngineSelect: () => calls.push(['engine', state.selectedBrowserEngineId]),
    syncExitSelect: () => calls.push(['exit', state.selectedRemoteExitId]),
    showStatus: message => calls.push(['status-message', message]),
    setLoading: value => calls.push(['loading', value]),
    startPageHeartbeat: () => calls.push(['heartbeat']), startPageStatusPolling: () => calls.push(['poll']),
    syncDisplayInputFromSession: value => calls.push(['input-transport', value.mode]),
    viewFromDisplaySession: value => ({ width: value.width, height: value.height }),
    syncViewFromResponse: () => {}, handleFileChooserFromStatus: () => {}, updateMetricsNode: () => {},
    syncBrowserLocation: url => calls.push(['url', url]),
    connectRemoteDisplay: async (display, page) => calls.push(['connect', page.page_id, page.actual_url, display]),
    closeRemoteDisplay: () => calls.push(['detach']),
    failRuntimeOwnedPage: async (...args) => { failures.push(args); return { state: 'pending' }; },
    fetchJson: async (path, options) => { requests.push(path); if (options?.body?.type === 'display_attach') return {
      schema: 'elastos.browser.display-attach-result/v1', page_id: 'page-one', request_id: options.body.request_id,
      previous_display_generation: options.body.display_generation, display_generation: 'display:' + '2'.repeat(32),
      initial_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'fresh-video' },
      audio_offer: { schema: 'elastos.browser.webrtc-offer/v1', type: 'offer', sdp: 'fresh-audio' },
    }; return { schema: 'elastos.browser.page-status/v1', page_id: 'page-one',
      actual_url: 'https://current.invalid/form', title: 'Retained form', direct_network: false,
      display_session: { mode: 'webrtc_remote_display', width: 1280, height: 720, source: 'diagnostic-status', ice_servers: [{ credential_present: true }] } }; },
  });
  for (const name of ['currentRuntimePageOwner', 'runtimeViewerOwnerActive', 'recoverableRuntimePage', 'fetchPageStatus', 'attachRecoveredDisplay', 'restoreRuntimePageViewer', 'settleRemoteDisplayFailure']) {
    vm.runInContext(declaration(name), state);
  }
  return { state, calls, failures, requests, restore: value => state.restoreRuntimePageViewer(value) };
}

test('startup restores exact owner, fresh URL, and Runtime-selected services without opening or closing', async () => {
  const h = harness(), original = summary();
  assert.equal(await h.restore(original), true);
  assert.equal(h.state.currentPage.page_id, 'page-one');
  assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
  assert.equal(h.state.currentPageGeneration, 1);
  assert.equal(h.state.currentPage.actual_url, 'https://current.invalid/form');
  assert.equal(h.state.runtimeOwnershipTerminallyAbsent, false);
  assert.deepEqual(h.requests, ['/api/apps/browser/pages/page-one/status', '/api/apps/browser/pages/page-one/webrtc']);
  assert.deepEqual(h.calls.find(row => row[0] === 'engine'), ['engine', 'selected-engine']);
  assert.deepEqual(h.calls.find(row => row[0] === 'exit'), ['exit', 'selected-exit']);
  const connection = h.calls.find(row => row[0] === 'connect');
  assert.equal(connection[2], 'https://current.invalid/form');
  assert.equal(connection[3].runtime_turn, original.sessions.recoverable_page.engine_page.display_session.runtime_turn);
  assert.equal(connection[3].display_generation, 'display:' + '2'.repeat(32));
  assert.equal(connection[3].initial_offer.sdp, 'fresh-video');
  assert.equal(connection[3].runtime_turn.generation, 'owned-generation');
  assert.deepEqual(h.failures, []);
});

test('restored page becomes visible to Home after its fresh address is applied', async () => {
  const h = harness(), pending = deferred(), fetch = h.state.fetchJson;
  h.state.fetchJson = async (path, options) => {
    if (path.endsWith('/status')) await pending.promise;
    return fetch(path, options);
  };
  const restoring = h.restore(summary());
  try {
    assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
    assert.equal(h.calls.some(row => row[0] === 'publish'), false);
  } finally { pending.resolve(); await restoring; }
  const url = h.calls.findIndex(row => row[0] === 'url');
  const publish = h.calls.findIndex(row => row[0] === 'publish');
  assert.ok(url >= 0 && publish > url);
  assert.ok(h.calls.findIndex(row => row[0] === 'connect') > publish);
});

test('Automatic Engine and local Exit remain the Runtime-selected empty values', async () => {
  const h = harness(), value = summary(); value.sessions.recoverable_page.service_selection.engine_id = '';
  value.sessions.recoverable_page.service_selection.exit_id = '';
  await h.restore(value);
  assert.equal(h.state.currentBrowserEngineId, ''); assert.equal(h.state.currentRemoteExitId, '');
});

test('only an explicit empty owned-page result permits a fresh startup open', async () => {
  for (const value of [null, {}, { sessions: {} }, { sessions: { schema: 'elastos.browser.session-capacity/v1' } }]) {
    const h = harness(); await assert.rejects(h.restore(value)); assert.deepEqual(h.requests, []); assert.equal(h.state.currentPage, null);
  }
  const h = harness(), empty = summary(); empty.sessions.recoverable_page = null;
  assert.equal(await h.restore(empty), false); assert.deepEqual(h.calls, []);
});

for (const [name, mutate] of [
  ['wrong page', value => { value.engine_page.page_id = 'foreign-page'; }],
  ['invalid cleanup', value => { value.cleanup.id = '../bad'; }],
  ['missing selection', value => { delete value.service_selection; }],
  ['invalid selection schema', value => { value.service_selection.schema = 'unknown'; }],
  ['invalid engine selection', value => { value.service_selection.engine_id = null; }],
  ['invalid exit selection', value => { value.service_selection.exit_id = 123; }],
  ['invalid page schema', value => { value.engine_page.schema = 'unknown'; }],
  ['unknown owner state', value => { value.state = 'unknown'; }],
]) test(`${name} preserves Runtime ownership without dispatch or viewer adoption`, async () => {
  const h = harness(), value = summary(); mutate(value.sessions.recoverable_page);
  await assert.rejects(h.restore(value)); assert.equal(h.state.currentPage, null); assert.deepEqual(h.requests, []); assert.deepEqual(h.failures, []);
});

test('cleanup-pending authority is retained for explicit close even without an Engine page', async () => {
  const h = harness(), value = summary(); value.sessions.recoverable_page.state = 'cleanup_pending';
  delete value.sessions.recoverable_page.engine_page; delete value.sessions.recoverable_page.service_selection;
  assert.equal(await h.restore(value), true); assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
  assert.deepEqual(h.requests, []); assert.equal(h.calls.some(row => row[0] === 'connect'), false); assert.deepEqual(h.failures, []);
});

for (const flag of ['unloadCleanupStarted', 'homeWindowCloseInFlight', 'homeWindowTerminalCloseConfirmed']) {
  test(`${flag} cancels startup adoption`, async () => {
    const h = harness(); h.state[flag] = true;
    assert.equal(await h.restore(summary()), true); assert.equal(h.state.currentPage, null); assert.deepEqual(h.requests, []);
  });
}

for (const transition of ['new-owner', 'unload']) test(`status completed after ${transition} cannot attach the old viewer`, async () => {
  const h = harness(), wait = deferred(); h.state.fetchJson = async () => wait.promise;
  const restoring = h.restore(summary());
  if (transition === 'unload') h.state.unloadCleanupStarted = true;
  else { h.state.currentPage = { ...h.state.currentPage, page_id: 'page-two' }; h.state.currentPageGeneration++; }
  wait.resolve({ schema: 'elastos.browser.page-status/v1', page_id: 'page-one', direct_network: false,
    actual_url: 'https://late.invalid/', display_session: { mode: 'webrtc_remote_display' } });
  await restoring;
  assert.equal(h.calls.some(row => row[0] === 'connect'), false);
  assert.deepEqual(h.failures, []);
});

test('a failed restore negotiation retains the page and explicit cleanup authority', async () => {
  const h = harness(); h.state.connectRemoteDisplay = async () => { throw new Error('negotiation failed'); };
  await assert.rejects(h.restore(summary()), /negotiation failed/);
  assert.equal(h.state.currentPage.page_id, 'page-one'); assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
  await h.state.settleRemoteDisplayFailure('first frame timed out');
  assert.deepEqual(h.failures, []); assert.ok(h.calls.some(row => row[0] === 'detach'));
});

test('a restore callback cannot change a later owner failure policy', async () => {
  const h = harness(); await h.restore(summary()); h.state.currentPageGeneration++;
  await h.state.settleRemoteDisplayFailure('new acquisition failure'); assert.equal(h.failures.length, 1);
});

for (const owned of [true, false]) test(`actual startup selects ${owned ? 'restore' : 'fresh open'} from Runtime ownership`, async () => {
  const h = harness(), value = summary(), opens = [];
  if (!owned) value.sessions.recoverable_page = null;
  Object.assign(h.state, { params: new URLSearchParams(), DEFAULT_URL: 'https://default.invalid/', addressInput: {},
    fetchBrowserSummary: async () => value, requestRuntimeOpen: async url => opens.push(url),
    isAuthoritySessionError: () => false, friendlyOpenError: error => error.message });
  await vm.runInContext(source.slice(source.lastIndexOf('const initialUrl =')), h.state);
  assert.deepEqual(opens, owned ? [] : ['https://default.invalid/']);
});


test('summary authority failure reaches startup renewal without a replacement open', async () => {
  const h = harness(), renewals = [], opens = [], error = Object.assign(new Error('Browser authority expired'), { status: 401 });
  Object.assign(h.state, { browserSummaryPromise: null, browserSummary: null, browserInstanceId: 'instance-one',
    params: new URLSearchParams(), DEFAULT_URL: 'https://default.invalid/', addressInput: {},
    fetchJson: async () => { throw error; },
    requestRuntimeOpen: async value => opens.push(value),
    isAuthoritySessionError: value => value.status === 401,
    friendlyOpenError: value => value.message,
    requestHomeRelaunch: value => { renewals.push(value); return true; } });
  vm.runInContext(declaration('fetchBrowserSummary'), h.state);
  await vm.runInContext(source.slice(source.lastIndexOf('const initialUrl =')), h.state);
  assert.deepEqual(renewals, ['Browser authority expired']);
  assert.deepEqual(opens, []); assert.deepEqual(h.failures, []);
  assert.equal(h.state.browserSummaryPromise, null);
});

for (const state of ['unavailable', 'missing', 'ownership-pending', 'absence-unreported']) test(`empty ${state} summary cannot start a page`, async () => {
  const h = harness(), value = summary(); value.sessions.recoverable_page = null;
  if (state === 'ownership-pending') value.sessions.fresh_start_allowed = false;
  else if (state === 'absence-unreported') delete value.sessions.fresh_start_allowed;
  else if (state === 'missing') delete value.sessions.status;
  else { value.sessions.status = 'unavailable'; value.sessions.engine_cleanup_obligations = 1; }
  await assert.rejects(h.restore(value)); assert.deepEqual(h.calls, []);
});
for (const flag of ['unloadCleanupStarted', 'homeWindowCloseInFlight', 'homeWindowTerminalCloseConfirmed']) {
  test(`delayed empty summary after ${flag} cannot start a page`, async () => {
    const h = harness(), wait = deferred(), value = summary(), opens = [];
    value.sessions.recoverable_page = null;
    Object.assign(h.state, { params: new URLSearchParams(), DEFAULT_URL: 'https://default.invalid/', addressInput: {},
      fetchBrowserSummary: async () => wait.promise, requestRuntimeOpen: async url => opens.push(url),
      isAuthoritySessionError: () => false, friendlyOpenError: error => error.message });
    const startup = vm.runInContext(source.slice(source.lastIndexOf('const initialUrl =')), h.state);
    h.state[flag] = true; wait.resolve(value); await startup;
    assert.deepEqual(opens, []); assert.deepEqual(h.failures, []);
    assert.equal(h.state.currentPage, null);
  });
}

test('diagnostic status cannot replace missing recovered display authority', async () => {
  const h = harness(), value = summary(); delete value.sessions.recoverable_page.engine_page.display_session;
  await assert.rejects(h.restore(value), /could not restore the Browser display/);
  assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
  assert.equal(h.calls.some(row => row[0] === 'connect'), false);
  assert.deepEqual(h.failures, []);
});

for (const missing of ['runtime-capability', 'engine-generation']) test(`${missing} preserves ownership and requests no attachment`, async () => {
  const h = harness(), value = summary();
  if (missing === 'runtime-capability') delete value.engine_adapter.display_attach_supported;
  else delete value.sessions.recoverable_page.engine_page.display_session.display_generation;
  await assert.rejects(h.restore(value), /needs an update/);
  assert.deepEqual(h.requests, ['/api/apps/browser/pages/page-one/status']);
  assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
});

test('a new document reconciles the Runtime pending attachment with its original request identity', async () => {
  const h = harness(), value = summary(), calls = [], fetch = h.state.fetchJson;
  value.sessions.recoverable_page.display_attachment = { schema: 'elastos.browser.display-attachment/v1', state: 'pending',
    request_id: 'e'.repeat(32), previous_display_generation: 'display:' + '1'.repeat(32) };
  h.state.fetchJson = async (path, options) => { if (options?.body) calls.push(options.body); return fetch(path, options); };
  await h.restore(value);
  assert.equal(calls.length, 1); assert.equal(calls[0].request_id, 'e'.repeat(32));
  assert.equal(calls[0].display_generation, value.sessions.recoverable_page.display_attachment.previous_display_generation);
});

for (const field of ['page_id', 'request_id', 'previous_display_generation', 'display_generation', 'initial_offer', 'audio_offer']) {
  test(`invalid attachment result ${field} retains owner without connecting`, async () => {
    const h = harness(), fetch = h.state.fetchJson;
    h.state.fetchJson = async (path, options) => {
      const result = await fetch(path, options);
      if (options?.body?.type === 'display_attach') result[field] = 'invalid';
      return result;
    };
    await assert.rejects(h.restore(summary()), /could not restore/);
    assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
    assert.equal(h.calls.some(row => row[0] === 'connect'), false); assert.deepEqual(h.failures, []);
  });
}

for (const transition of ['unload', 'new-owner']) test(`late attachment after ${transition} cannot replace the display`, async () => {
  const h = harness(), fetch = h.state.fetchJson, wait = deferred(), started = deferred();
  h.state.fetchJson = async (path, options) => {
    const result = await fetch(path, options);
    if (options?.body?.type === 'display_attach') { started.resolve(); await wait.promise; }
    return result;
  };
  const pending = h.restore(summary()); await started.promise;
  if (transition === 'unload') h.state.unloadCleanupStarted = true;
  else { h.state.currentPage = { ...h.state.currentPage, page_id: 'page-two' }; h.state.currentPageGeneration++; }
  wait.resolve(); await pending;
  assert.equal(h.calls.some(row => row[0] === 'connect'), false);
  assert.equal(h.state.currentPage.display_session.display_generation, 'display:' + '1'.repeat(32));
});

for (const stage of ['status', 'attachment']) for (const transition of ['unload', 'new-owner']) {
  test(`late ${stage} rejection after ${transition} cannot fail a newer viewer`, async () => {
    const h = harness(), fetch = h.state.fetchJson, wait = deferred(), started = deferred();
    h.state.fetchJson = async (path, options) => {
      if ((stage === 'attachment') === (options?.body?.type === 'display_attach')) {
        started.resolve(); return wait.promise;
      }
      return fetch(path, options);
    };
    const pending = h.restore(summary()); await started.promise;
    if (transition === 'unload') h.state.unloadCleanupStarted = true;
    else { h.state.currentPage = { ...h.state.currentPage, page_id: 'page-two' }; h.state.currentPageGeneration++; }
    const before = h.calls.length;
    wait.reject(Object.assign(new Error('late authority error'), { status: 401 }));
    assert.equal(await pending, true);
    assert.equal(h.calls.length, before); assert.deepEqual(h.failures, []);
    assert.equal(h.calls.some(row => row[0] === 'connect'), false);
  });
}

for (const flag of ['unloadCleanupStarted', 'homeWindowCloseInFlight', 'homeWindowTerminalCloseConfirmed']) {
  test(`delayed summary rejection after ${flag} cannot renew or change a closing viewer`, async () => {
    const h = harness(), wait = deferred(), renewals = [];
    Object.assign(h.state, { params: new URLSearchParams(), DEFAULT_URL: 'https://default.invalid/', addressInput: {},
      fetchBrowserSummary: async () => wait.promise, requestRuntimeOpen: async () => assert.fail('fresh open'),
      isAuthoritySessionError: () => true, friendlyOpenError: error => error.message,
      requestHomeRelaunch: value => { renewals.push(value); return true; } });
    const startup = vm.runInContext(source.slice(source.lastIndexOf('const initialUrl =')), h.state);
    h.state[flag] = true; const before = h.calls.length;
    wait.reject(new Error('late expired authority')); await startup;
    assert.deepEqual(renewals, []); assert.deepEqual(h.failures, []); assert.equal(h.calls.length, before);
  });
}

for (const stage of ['status', 'attachment']) for (const closeState of ['in-flight', 'pending', 'delivery-pending']) {
  for (const outcome of ['success', 'failure']) test(`${stage} ${outcome} during ${closeState} close cannot restore media or enable controls`, async () => {
    const h = harness(), fetch = h.state.fetchJson, wait = deferred(), started = deferred();
    let attaches = 0;
    h.state.fetchJson = async (path, options) => {
      const attaching = options?.body?.type === 'display_attach';
      if (attaching) attaches++;
      const result = await fetch(path, options);
      if ((stage === 'attachment') === attaching) { started.resolve(); await wait.promise; }
      return result;
    };
    const pending = h.restore(summary()); await started.promise;
    const owner = h.state.currentRuntimePageOwner();
    if (closeState === 'in-flight') h.state.homeWindowCloseInFlight = true;
    if (closeState === 'pending') h.state.runtimePageCleanup.status = value =>
      sameRuntimePageOwner(value, owner) ? { pending: true, in_flight: false, attempts: 1 } : null;
    if (closeState === 'delivery-pending') h.state.pendingHomeWindowCloseDelivery = { owner };
    const before = h.calls.length;
    if (outcome === 'failure') wait.reject(new Error('late close-time failure')); else wait.resolve();
    assert.equal(await pending, true);
    assert.equal(attaches, 1); // Both requests started before close began.
    assert.equal(h.calls.length, before); assert.deepEqual(h.failures, []);
    assert.equal(h.state.currentPage.display_session.display_generation, 'display:' + '1'.repeat(32));
    assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
  });
}

test('cleanup for a different owner leaves this viewer eligible', async () => {
  const h = harness();
  h.state.pendingHomeWindowCloseDelivery = { owner: { page_id: 'other', generation: 7, runtime_cleanup: { schema: 'elastos.browser.cleanup-handle/v1', id: 'old-cleanup' } } };
  await h.restore(summary()); assert.equal(h.calls.some(row => row[0] === 'connect'), true);
});

test('restore starts attachment while fresh status is pending and publishes only after both validate', async () => {
  const h = harness(), status = deferred(), attachment = deferred(), fetch = h.state.fetchJson, started = [];
  h.state.fetchJson = async (path, options) => {
    started.push(path.endsWith('/status') ? 'status' : 'attachment');
    await (path.endsWith('/status') ? status.promise : attachment.promise);
    return fetch(path, options);
  };
  const pending = h.restore(summary());
  try {
    assert.deepEqual(started, ['status', 'attachment']);
    attachment.resolve();
    await new Promise(resolve => setImmediate(resolve));
    assert.equal(h.calls.some(row => ['publish', 'connect'].includes(row[0])), false);
  } finally { status.resolve(); attachment.resolve(); await pending; }
  assert.equal(h.calls.filter(row => row[0] === 'connect').length, 1);
});

for (const field of ['schema', 'page_id', 'direct_network']) test(`invalid concurrent status ${field} cannot publish or connect`, async () => {
  const h = harness(), fetch = h.state.fetchJson;
  h.state.fetchJson = async (path, options) => {
    const result = await fetch(path, options);
    if (path.endsWith('/status')) result[field] = 'invalid';
    return result;
  };
  await assert.rejects(h.restore(summary()));
  assert.equal(h.calls.some(row => ['publish', 'connect'].includes(row[0])), false);
  assert.equal(h.state.currentPage.runtime_cleanup.id, 'cleanup-one');
});

test('attachment failure while status is pending cannot publish after late valid status', async () => {
  const h = harness(), status = deferred(), fetch = h.state.fetchJson;
  h.state.fetchJson = async (path, options) => {
    if (path.endsWith('/webrtc')) throw new Error('attachment failed');
    await status.promise;
    return fetch(path, options);
  };
  const pending = h.restore(summary());
  // Release in the next turn so both old and new scheduling finish without a test hang.
  setImmediate(() => status.resolve());
  await assert.rejects(pending, /attachment failed/);
  await new Promise(resolve => setImmediate(resolve));
  assert.equal(h.calls.some(row => ['publish', 'connect'].includes(row[0])), false);
  assert.equal(h.state.currentPage.display_session.display_generation, 'display:' + '1'.repeat(32));
});
