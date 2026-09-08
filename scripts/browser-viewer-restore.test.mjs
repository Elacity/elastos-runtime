import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';
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
  return { sessions: { schema: 'elastos.browser.session-capacity/v1', status: 'configured', capacity_available: true, fresh_start_allowed: true, recoverable_page: {
    schema: 'elastos.browser.recoverable-page/v1', state: 'active', page_id: 'page-one',
    cleanup: { schema: 'elastos.browser.cleanup-handle/v1', id: 'cleanup-one' },
    service_selection: { schema: 'elastos.browser.service-selection/v1', engine_id: 'selected-engine', exit_id: 'selected-exit' },
    engine_page: { schema: 'elastos.browser.engine.page/v1', page_id: 'page-one', actual_url: 'https://old.invalid/',
      adapter: 'actual-engine', display_session: { mode: 'webrtc_remote_display', width: 1280, height: 720, source: 'runtime-recovery', runtime_turn: { generation: 'owned-generation' } } },
  } } };
}
function harness() {
  const calls = [], failures = [], requests = [];
  const state = vm.createContext({ currentPage: null, currentPageGeneration: 0, nextPageGeneration: 1,
    restoredViewerOwner: null, unloadCleanupStarted: false, relaunchRequested: false,
    homeWindowCloseInFlight: false, homeWindowTerminalCloseConfirmed: false,
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
    fetchJson: async path => { requests.push(path); return { schema: 'elastos.browser.page-status/v1', page_id: 'page-one',
      actual_url: 'https://current.invalid/form', title: 'Retained form', direct_network: false,
      display_session: { mode: 'webrtc_remote_display', width: 1280, height: 720, source: 'diagnostic-status', ice_servers: [{ credential_present: true }] } }; },
  });
  for (const name of ['currentRuntimePageOwner', 'recoverableRuntimePage', 'fetchPageStatus', 'restoreRuntimePageViewer', 'settleRemoteDisplayFailure']) {
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
  assert.deepEqual(h.requests, ['/api/apps/browser/pages/page-one/status']);
  assert.deepEqual(h.calls.find(row => row[0] === 'engine'), ['engine', 'selected-engine']);
  assert.deepEqual(h.calls.find(row => row[0] === 'exit'), ['exit', 'selected-exit']);
  const connection = h.calls.find(row => row[0] === 'connect');
  assert.equal(connection[2], 'https://current.invalid/form');
  assert.equal(connection[3], original.sessions.recoverable_page.engine_page.display_session);
  assert.equal(connection[3].runtime_turn.generation, 'owned-generation');
  assert.deepEqual(h.failures, []);
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
