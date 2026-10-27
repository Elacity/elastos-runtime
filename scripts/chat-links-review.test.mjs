import assert from 'node:assert/strict';
import { readFileSync } from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const read = path => readFileSync(new URL(`../${path}`, import.meta.url), 'utf8');
test('Community attachment fixture permits the upload preflight', () => {
  const source = read('scripts/chat-room-configured-layout-smoke.mjs');
  const start = source.indexOf('if (request.method === "OPTIONS") {');
  const block = source.slice(start, source.indexOf('\n      }', start) + 8);
  let headers, status;
  vm.runInNewContext(`(() => { ${block} })()`, {
    request: { method: 'OPTIONS' },
    response: { writeHead(code, value) { status = code; headers = value; return { end() {} }; } },
  });
  assert.equal(status, 204);
  assert.ok(headers['access-control-allow-methods'].split(',').includes('PUT'));
  assert.ok(headers['access-control-allow-headers'].split(',').includes('x-elastos-upload-offset'));
});

test('Home smoke exercises guarded shared-file opens from authorized apps', () => {
  const source = read('scripts/home-camofox-smoke.mjs');
  assert.doesNotMatch(source, /preferredViewer/);
  assert.match(source, /linkCard\.href === ""/);
  assert.match(source, /linkCard\.role === "link"/);
  assert.match(source, /async function confirmSharedFile\(tabId, uri, allowMissing = false\)/);
  assert.equal((source.match(/confirmSharedFile\(tabId, uri(?:, true)?\)/g) || []).length, 3);
  assert.match(source, /await click\(tabId, "#home-link-open"\)/);
});

test('Home routes every URI request through the shared-file authority check', () => {
  const source = read('capsules/home/browser/home-shell-host.js');
  const start = source.indexOf('  if (data.type === "home:open-uri") {');
  const block = source.slice(start, source.indexOf('\n  if (data.type ===', start + 1));
  let calls = 0;
  vm.runInNewContext(`(() => { ${block} })()`, {
    data: { type: 'home:open-uri', uri: 'elastos://cid' },
    context: { kind: 'home', targetId: 'home-gui' },
    homeExternalLinkPrompt: {}, fetchJson() {},
    canOpenUriFromHomeMessage: () => true, resolveOpenUri: () => null, console: { warn() {} },
    requestHomeSharedContent() { calls++; },
  });
  assert.equal(calls, 1);
  for (const obsolete of ['SHELL_MESSAGE_OPEN_URI_SOURCES', 'canOpenUriFromHomeMessage', 'resolveOpenUri', 'resolvePeerInviteUri']) {
    assert.ok(!source.includes(obsolete), obsolete);
  }
});

test('Home and Library import the shared-link revisions through fresh cache keys', () => {
  for (const file of ['home-shell-host.js', 'shell-auth.js', 'home-wallet-connector-host.js']) {
    assert.ok(read(`capsules/home/browser/${file}`).includes('./shell-core.js?v=home-links-20261027a'), file);
  }
  assert.ok(read('capsules/home/browser/index.html').includes('./home-shell-host.js?v=home-links-20261027a'));
  assert.ok(read('capsules/library/browser/index.html').includes('src/app.js?v=library-links-20261027a'));
});
