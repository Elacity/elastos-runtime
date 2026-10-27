import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('./app.js', import.meta.url), 'utf8');
function fn(name) {
  const start = source.indexOf(`    ${name === 'boot' || name === 'loadRoots' ? 'async ' : ''}function ${name}(`);
  return source.slice(start, source.indexOf('\n    }', start) + 6);
}
for (const initialUri of ['elastos://shared-cid', 'localhost://home/Documents/subfolder']) {
  test(`Library chooses a valid starting folder for ${initialUri}`, async () => {
    const state = { initialUri, currentUri: '', roots: [] };
    const roots = [{ id: 'documents', uri: 'localhost://home/Documents' }];
    const context = vm.createContext({ state, providerApi: async () => ({ roots }), orderRoots: x => x, renderPlaces() {} });
    vm.runInContext([fn('activeRootForUri'), fn('rootForUri'), fn('loadRoots')].join('\n'), context);
    await vm.runInContext('loadRoots()', context);
    assert.equal(state.currentUri, initialUri.startsWith('localhost://') ? initialUri : roots[0].uri);
  });
}
for (const failure of ['roots', 'folder', 'none']) {
  test(`Library offers the shared file when ${failure} fails`, async () => {
    const node = () => ({ classList: { remove() {} }, dataset: {}, innerHTML: '' });
    const elements = { lockedShell: node(), libraryShell: node(), content: node(), dialog: node() };
    const context = vm.createContext({ elements, state: { homeToken: 'token', mode: 'browse' },
      queryParams: new URLSearchParams('cid=shared-cid'), sharedFileMarkup: cid => `download ${cid}`,
      loadRoots: async () => { if (failure === 'roots') throw Error('roots'); },
      loadCurrentFolder: async () => { if (failure === 'folder') throw Error('folder'); },
      ...Object.fromEntries(['setSidebarWidth', 'syncModeChrome', 'bindEvents', 'announceHomeChrome',
        'installBrowserHistory', 'setStatus', 'scheduleRootPrefetch', 'startLibraryEventStream', 'showError'].map(key => [key, () => {}])),
    });
    vm.runInContext(fn('boot'), context);
    await vm.runInContext('boot()', context);
    assert.equal(elements.dialog.innerHTML, 'download shared-cid');
  });
}
