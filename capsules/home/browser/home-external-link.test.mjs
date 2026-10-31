import assert from 'node:assert/strict';
import test from 'node:test';
import { createHomeExternalLinkPrompt } from './home-external-link.js';

const cid = 'bafybeigdyrzt5sfp7udm7hu76uh7y26nf3w2pz3hftb5xg3mh5g4xq7y5u';
const uri = `elastos://${cid}`;
const result = (state = 'available') => ({ schema: 'elastos.home.shared-content.result/v1',
  cid, uri, state, check_id: 'check-1', sender_display_name: 'Alma', size_bytes: 30 });
function harness() {
  let clock = 0;
  const document = {};
  function node() {
    const listeners = new Map();
    return { hidden: false, disabled: false, textContent: '', setAttribute() {},
      addEventListener(type, fn) { listeners.set(type, [...(listeners.get(type) || []), fn]); },
      focus() { document.activeElement = this; },
      fire(type, event = {}) { for (const fn of listeners.get(type) || []) fn({ target: this, ...event }); } };
  }
  const nodes = Object.fromEntries(['root', 'appName', 'host', 'address', 'openButton', 'cancelButton',
    'eyebrow', 'title', 'asker', 'size', 'status'].map(key => [key, node()]));
  const opened = [], gets = [];
  let live = true, otherSheet = false;
  const windowRef = { ...node(), document, location: { origin: 'https://home.test' },
    performance: { now: () => clock }, open: (...args) => opened.push(args) };
  const prompt = createHomeExternalLinkPrompt({ ...nodes, windowRef, isLive: () => live,
    otherSheetShown: () => otherSheet, openSharedFile: async value => opened.push(value) });
  const begin = (extra = {}) => prompt.begin({ homeToken: 'launch', appId: 'chat-room', kind: 'shared', uri,
    check: async () => result(), get: async (id, signal) => { gets.push({ id, signal }); return { state: 'local' }; }, ...extra });
  return { ...nodes, prompt, begin, opened, gets, windowRef, document, tick: (ms = 600) => { clock += ms; },
    live: value => { live = value; }, other: value => { otherSheet = value; },
    click: (trusted = true) => nodes.openButton.fire('click', { isTrusted: trusted }) };
}
const settle = () => new Promise(resolve => setImmediate(resolve));

test('shared file asks one short question naming the sender, with the stated size', () => {
  const h = harness();
  assert.equal(h.prompt.show(h.begin(), result()), true);
  assert.equal(h.eyebrow.hidden, true);
  assert.equal(h.title.textContent, 'Get this file from Alma?');
  assert.equal(h.address.hidden, true);
  assert.equal(h.address.textContent, '');
  assert.equal(h.host.hidden, true);
  assert.equal(h.size.textContent, '30 bytes');
  assert.equal(h.status.hidden, true);
  assert.equal(h.openButton.textContent, 'Get');
  assert.equal(h.cancelButton.textContent, 'Cancel');
  assert.equal(h.begin(), '');
});

test('the guard and trusted click protect Get; local success opens once', async () => {
  const h = harness(); h.prompt.show(h.begin(), result());
  h.click(); h.tick(); h.click(false); assert.equal(h.gets.length, 0);
  h.click(); h.click(); await settle();
  assert.equal(h.gets.length, 1); assert.equal(h.gets[0].id, 'check-1');
  assert.equal(h.opened.length, 1); assert.equal(h.opened[0].cid, cid);
  assert.equal(h.root.hidden, true);
});

test('already local opens without a get and an unknown size shows nothing', async () => {
  const h = harness(), local = result('local'); delete local.size_bytes; delete local.check_id;
  assert.equal(h.prompt.show(h.begin(), local), true);
  assert.equal(h.title.textContent, 'Open this file?');
  assert.equal(h.openButton.textContent, 'Open');
  assert.equal(h.size.hidden, true);
  h.tick(); h.click(); await settle(); assert.equal(h.gets.length, 0); assert.equal(h.opened.length, 1);
});

test('a non-contact sees one plain line and OK', () => {
  const h = harness(); h.prompt.show(h.begin(), result('not_contact'));
  assert.equal(h.title.textContent, 'Only contacts can send you files');
  assert.equal(h.status.textContent, 'Add Alma as a contact to get this file.');
  assert.equal(h.size.hidden, true);
  assert.equal(h.openButton.hidden, true);
  assert.equal(h.cancelButton.textContent, 'OK');
  h.cancelButton.fire('click'); assert.equal(h.root.hidden, true); assert.equal(h.opened.length, 0);
});

for (const [state, copy, retry] of [
  ['sender_offline', 'Alma is offline. Try again later.', true], ['not_contact', 'Add Alma as a contact to get this file.', false],
  ['not_here', "This file isn't on your Home.", false], ['not_shared', 'Alma no longer shares this file.', false],
  ['too_large', 'This file is too large to get.', false], ['mismatch', "didn't match its link", false],
  ['timeout', 'This took too long. Try again.', true],
]) test(`shared file state ${state} has clear copy and action`, async () => {
  const h = harness();
  if (['not_contact', 'not_here'].includes(state)) h.prompt.show(h.begin(), result(state));
  else { h.prompt.show(h.begin({ get: async () => ({ state }) }), result()); h.tick(); h.click(); await settle(); }
  assert.ok(h.status.textContent.includes(copy), h.status.textContent);
  assert.equal(h.openButton.disabled, !retry);
  assert.equal(h.opened.length, 0);
});

test('retry checks again before redeeming a fresh check id', async () => {
  const h = harness(); let checks = 0;
  h.prompt.show(h.begin({ get: async (id) => { h.gets.push({ id }); return { state: 'sender_offline' }; },
    check: async () => { checks++; return { ...result(), check_id: 'check-2' }; } }), result());
  h.tick(); h.click(); await settle(); h.gets.length = 0;
  h.tick(); h.click(); await settle(); assert.equal(checks, 1); assert.equal(h.gets.length, 0);
  h.click(); assert.equal(h.gets.length, 0); h.tick(); h.click(); await settle();
  assert.equal(h.gets[0].id, 'check-2');
});

test('Cancel aborts Get, drops its late result and applies the shared cooldown', async () => {
  const h = harness(); let finish, signal;
  h.prompt.show(h.begin({ get: (_id, s) => { signal = s; return new Promise(resolve => { finish = resolve; }); } }), result());
  h.tick(); h.click(); assert.equal(h.openButton.textContent, 'Getting…');
  assert.equal(h.openButton.hidden, false); assert.equal(h.openButton.disabled, true);
  assert.equal(h.cancelButton.textContent, 'Cancel');
  h.cancelButton.fire('click'); assert.equal(signal.aborted, true); assert.equal(h.root.hidden, true);
  finish({ state: 'local' }); await settle(); assert.equal(h.opened.length, 0);
  assert.equal(h.begin(), ''); h.tick(3000); assert.ok(h.begin());
});

test('retirement and a second Home sheet prevent stale opens', async () => {
  const h = harness(); h.other(true); assert.equal(h.prompt.show(h.begin(), result()), false);
  h.other(false); h.prompt.show(h.begin(), result()); h.prompt.retire('launch');
  h.tick(); h.click(); assert.equal(h.opened.length, 0);
  const id = h.begin(); h.live(false); assert.equal(h.prompt.show(id, result()), false);
});

test('malformed and swapped check results fail closed', () => {
  for (const change of [{ uri: `${uri}/path` }, { cid: 'other' }, { state: 'unexpected' },
    { check_id: undefined }, { size_bytes: -1 }, { extra: 'value' }]) {
    const h = harness(); assert.equal(h.prompt.show(h.begin(), { ...result(), ...change }), false);
    assert.equal(h.opened.length, 0);
  }
});

test('web links keep their existing guarded synchronous opening', () => {
  const h = harness(); const id = h.prompt.begin({ homeToken: 'launch', appId: 'chat-room' });
  assert.ok(h.prompt.show(id, { schema: 'elastos.home.external-link.result/v1', app_id: 'chat-room', app_title: 'Chat', url: 'https://example.test/' }));
  h.click(); assert.equal(h.opened.length, 0); h.tick(); h.click();
  assert.deepEqual(h.opened, [['https://example.test/', '_blank', 'noopener,noreferrer']]);
});

const { requestHomeSharedContent } = await import('./home-external-link.js');
const context = { kind: 'app-frame', targetId: 'chat-room', homeToken: 'launch' };
const message = () => ({ type: 'home:open-uri', uri, homeToken: 'launch',
  source: { conversation_id: 'file-community', message_id: 'sha256:message' } });
test('the Home handoff checks the exact source with launch authority and gets with Home authority', async () => {
  const h = harness(), calls = [];
  const fetchJson = async (route, init) => { calls.push({ route, ...init, body: JSON.parse(init.body) });
    return route.endsWith('/check') ? result() : { state: 'local' }; };
  assert.equal(requestHomeSharedContent({ context, data: message(), prompt: h.prompt, fetchJson }), true);
  await settle();
  assert.deepEqual(calls[0].body, { schema: 'elastos.home.shared-content.check/v1', app_token: 'launch', uri, source: message().source });
  assert.equal(calls[0].headers, undefined);
  assert.equal(calls[0].route, '/api/apps/home/shared-content/check');
  assert.equal(h.gets.length, 0);
  h.tick(); h.click(); await settle();
  assert.equal(calls[1].route, '/api/apps/home/shared-content/get');
  assert.deepEqual(calls[1].body, { check_id: 'check-1' });
  assert.equal(calls[1].headers, undefined, 'fetchJson owns Home authority; the app token stays out of get');
  assert.equal(h.opened.length, 1);
});

test('the Home handoff refuses extra fields, malformed source and unauthorized apps', () => {
  for (const [sender, data] of [
    [context, { ...message(), preferredViewer: 'documents' }],
    ...['report.pdf', null, [], '', '../report.pdf', 'a\\b', 'a\nb', 'x'.repeat(256)].map(fileName => [context, { ...message(), fileName }]),
    [context, { ...message(), source: { ...message().source, cid } }],
    [context, { ...message(), source: null }], [context, { ...message(), source: [] }],
    [context, { ...message(), source: { conversation_id: 'file-community', message_id: '' } }],
    [context, { ...message(), homeToken: 'wrong' }],
    [{ ...context, targetId: 'documents' }, message()],
    [{ ...context, targetId: 'documents' }, (({ source, ...ownFile }) => ownFile)(message())],
    [{ ...context, targetId: 'wallet' }, message()], [{ ...context, kind: 'shell-frame' }, message()],
  ]) {
    const h = harness();
    assert.equal(requestHomeSharedContent({ context: sender, data, prompt: h.prompt,
      fetchJson: () => { throw new Error('unexpected request'); } }), false);
  }
});

test('Chat checks its own file without a source and a network error offers retry', async () => {
  const h = harness(), data = message(); delete data.source;
  const calls = [];
  assert.equal(requestHomeSharedContent({ context, data, prompt: h.prompt,
    fetchJson: async (route, init) => { calls.push(JSON.parse(init.body)); throw new Error('offline'); } }), true);
  assert.deepEqual(calls[0], { schema: 'elastos.home.shared-content.check/v1', app_token: 'launch', uri });
  await settle(); assert.match(h.status.textContent, /could not check/); assert.equal(h.openButton.disabled, false);
});

test('shared get uses the Home shell token while check keeps the app launch binding', async () => {
  const previousDocument = globalThis.document, previousFetch = globalThis.fetch;
  globalThis.document = { querySelector: () => null };
  try {
    const { fetchJson, setHomeAuthorityToken, clearHomeAuthorityToken } = await import('./shell-core.js');
    setHomeAuthorityToken('home-authority');
    const calls = [];
    globalThis.fetch = async (route, init) => { calls.push({ route, init }); return { ok: true, json: async () => ({}) }; };
    await fetchJson('/api/apps/home/shared-content/get', { method: 'POST', body: JSON.stringify({ check_id: 'checked' }) });
    await fetchJson('/api/apps/home/shared-content/check', { method: 'POST', body: JSON.stringify({ app_token: 'app-launch' }) });
    assert.equal(calls[0].init.headers['x-elastos-home-token'], 'home-authority');
    assert.equal(calls[1].init.headers['x-elastos-home-token'], 'home-authority');
    assert.equal(JSON.parse(calls[1].init.body).app_token, 'app-launch');
    clearHomeAuthorityToken();
  } finally { globalThis.document = previousDocument; globalThis.fetch = previousFetch; }
});


test('Tab keeps focus on Cancel when the shared file has no available action', () => {
  const h = harness(); h.prompt.show(h.begin(), result('not_contact'));
  h.windowRef.fire('keydown', { key: 'Tab', preventDefault() {} });
  assert.equal(h.document.activeElement, h.cancelButton);
});

test('stale presence still offers Get and only a failed Get reports offline', async () => {
  const h = harness();
  assert.equal(h.prompt.show(h.begin(), result('sender_offline')), true);
  assert.equal(h.openButton.textContent, 'Get');
  assert.equal(h.openButton.disabled, false);
  assert.equal(h.title.textContent, 'Get this file from Alma?');
  assert.equal(h.status.textContent, '');
  h.tick(); h.click(); await settle();
  assert.equal(h.gets.length, 1);
  assert.equal(h.opened.length, 1);
});

test('an offline presence hint requires a redeemable check id', () => {
  const h = harness();
  assert.equal(h.prompt.show(h.begin(), { ...result('sender_offline'), check_id: undefined }), false);
});

for (const [label, bytes, target] of [
  ['text', new TextEncoder().encode('Hello Alma'), 'documents'],
  ['PNG', Uint8Array.of(137, 80, 78, 71, 13, 10, 26, 10), 'library'],
  ['PDF', new TextEncoder().encode('%PDF-1.7\n'), 'library'],
  ['binary controls', Uint8Array.of(1, 2, 3), 'library'],
]) test(`local ${label} opens in ${target}`, async () => {
  const { openHomeSharedFile } = await import('./home-external-link.js');
  assert.equal(typeof openHomeSharedFile, 'function');
  const opens = [], reads = [];
  await openHomeSharedFile(result('local'), {
    fetchLocal: async (path, options) => {
      reads.push({ path, options });
      return new Response(bytes, { headers: { 'content-type': 'application/octet-stream' } });
    },
    openTarget: async (...args) => opens.push(args),
  });
  assert.equal(reads[0].path, `/content/${cid}`);
  assert.deepEqual(opens, [[target, { query: { cid, ...(target === 'documents' ? { uri, view: 'read' } : {}) } }]]);
});

test('shared file sheet uses the byte count shown in a Chat post', () => {
  for (const [bytes, size] of [[30, '30 bytes'], [1024, '1.0 KB'], [3328, '3.2 KB'], [3840, '3.8 KB'], [3565158, '3.4 MB'], [1073741824, '1.0 GB']]) {
    const h = harness(); h.prompt.show(h.begin(), { ...result(), size_bytes: bytes });
    assert.equal(h.size.textContent, size);
  }
});
test('an unnamed sender uses natural contact instructions', () => {
  const h = harness(); h.prompt.show(h.begin(), { ...result('not_contact'), sender_display_name: undefined });
  assert.equal(h.status.textContent, 'Add this person as a contact to get this file.');
});

test('Home opens binary shared content in Library by CID', async () => {
  const { openHomeSharedFile } = await import('./home-external-link.js');
  const opens = [];
  await openHomeSharedFile({ cid, uri }, {
    fetchLocal: async () => new Response('%PDF-1.7'),
    openTarget: async (...args) => opens.push(args) });
  assert.deepEqual(opens, [['library', { query: { cid } }]]);
});

test('a Chat post file name rides along as a title only, never in the check', async () => {
  for (const [name, expected] of [['omp-test-note.txt', 'omp-test-note.txt'], ['..x\u0007.txt', 'x.txt'],
    ['../x.txt', ''], ['a\\b', ''], ['n'.repeat(300), 'n'.repeat(200)], ['中'.repeat(80) + '.txt', '中'.repeat(65) + '.txt']]) {
    const h = harness(), calls = [];
    const fetchJson = async (route, init) => { calls.push(JSON.parse(init.body));
      return route.endsWith('/check') ? result() : { state: 'local' }; };
    assert.equal(requestHomeSharedContent({ context, data: { ...message(), name }, prompt: h.prompt, fetchJson }), true);
    await settle();
    assert.deepEqual(calls[0], { schema: 'elastos.home.shared-content.check/v1', app_token: 'launch', uri, source: message().source });
    h.tick(); h.click(); await settle();
    assert.equal(h.opened.length, 1);
    assert.equal(h.opened[0].cid, cid);
    assert.equal(h.opened[0].name, expected);
  }
  for (const name of [5, null, [], 'x'.repeat(2049)]) {
    const h = harness();
    assert.equal(requestHomeSharedContent({ context, data: { ...message(), name }, prompt: h.prompt,
      fetchJson: () => { throw new Error('unexpected request'); } }), false);
  }
});

test('Home passes the cleaned name to Documents and Library and never lets it pick the app', async () => {
  const { openHomeSharedFile, sharedFileName } = await import('./home-external-link.js');
  for (const [bytes, target] of [['Hello', 'documents'], ['%PDF-1.7', 'library']]) {
    for (const [name, expected] of [['report.pdf', 'report.pdf'], ['notes.txt', 'notes.txt'], ['../a', undefined], ['', undefined]]) {
      const opens = [];
      await openHomeSharedFile({ cid, uri, name }, {
        fetchLocal: async () => new Response(bytes), openTarget: async (...args) => opens.push(args) });
      assert.equal(opens[0][0], target, `${name} must not change the app`);
      assert.equal(opens[0][1].query.name, expected);
    }
  }
  assert.equal(sharedFileName(' .\u001b.hidden '), 'hidden');
  assert.equal(sharedFileName(undefined), '');
});
