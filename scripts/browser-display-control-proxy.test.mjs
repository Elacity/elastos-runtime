import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, rmSync } from 'node:fs';
import http from 'node:http';
import { tmpdir } from 'node:os';
import path from 'node:path';
import test from 'node:test';
import vm from 'node:vm';

const source = readFileSync(new URL('./browser-vm-control-service.mjs', import.meta.url), 'utf8');
function declaration(name) {
  const start = source.search(new RegExp(`(?:async )?function ${name}\\(`));
  const end = source.slice(start).search(/\n\}(?=\n|$)/);
  assert.ok(start >= 0 && end > 0, name);
  return source.slice(start, start + end + 2);
}
const routeStart = source.indexOf('      const pageWebrtcMatch =');
const routeEnd = source.indexOf('      if (req.method === "POST" && url.pathname === "/pages")', routeStart);
assert.ok(routeStart > 0 && routeEnd > routeStart);
const route = source.slice(routeStart, routeEnd);
const functions = ['safeId', 'validateAbsolutePath', 'readJsonBody', 'requestJsonOverUnix',
  'browserDisplayControlError', 'postJsonOverUnix', 'activePageGuestControl', 'proxyGuestPageWebrtc']
  .map(declaration).join('\n');
const cases = [
  ['display_attach_busy', 409], ['display_generation_mismatch', 409], ['display_owner_changed', 409],
  ['display_attach_unsupported', 501], ['display_attach_failed', 503], ['display_attach_uncertain', 503],
];

test('actual VM WebRTC HTTP route preserves each typed guest error across its Unix proxy', async t => {
  const scratch = mkdtempSync(path.join(tmpdir(), 'browser-display-proxy-'));
  const socket = path.join(scratch, 'guest.sock');
  let guestResult, guestStatus = 503;
  const guestRequests = [];
  const guest = http.createServer(async (req, res) => {
    let body = ''; for await (const chunk of req) body += chunk;
    guestRequests.push({ path: req.url, method: req.method, body: JSON.parse(body) });
    res.writeHead(guestStatus, { 'content-type': 'application/json' }); res.end(JSON.stringify(guestResult));
  });
  const context = vm.createContext({ http, Buffer, Error, config: { signal_timeout_ms: 500 },
    activePages: new Map([['owned-page', { page: { control_socket_path: socket } }]]), activeVms: new Map(),
  });
  vm.runInContext(functions, context);
  const handler = vm.runInContext(`(async function(req, url, sendJson) { ${route}\n sendJson(404, {}); })`, context);
  const proxy = http.createServer((req, res) => {
    handler(req, new URL(req.url, 'http://localhost'), (status, result) => {
      res.writeHead(status, { 'content-type': 'application/json' }); res.end(JSON.stringify(result));
    }).catch(error => { res.writeHead(500); res.end(error.message); });
  });
  t.after(async () => {
    for (const server of [proxy, guest]) {
      server.closeAllConnections();
      if (server.listening) await new Promise(resolve => server.close(resolve));
    }
    rmSync(scratch, { recursive: true, force: true });
  });
  await new Promise(resolve => guest.listen(socket, resolve));
  await new Promise(resolve => proxy.listen(0, '127.0.0.1', resolve));
  const endpoint = `http://127.0.0.1:${proxy.address().port}/pages/owned-page/webrtc`;
  const body = { schema: 'elastos.browser.display-attach-request/v1', type: 'display_attach',
    request_id: 'a'.repeat(32), display_generation: 'display:' + 'b'.repeat(32) };
  for (const [code, expectedStatus] of cases) {
    guestStatus = expectedStatus; guestResult = { code, error: 'guest-private-detail' };
    const response = await fetch(endpoint, { method: 'POST', body: JSON.stringify(body) });
    assert.equal(response.status, expectedStatus, code);
    assert.deepEqual(await response.json(), { code, error: 'Browser display operation failed.' }, code);
    assert.deepEqual(guestRequests.at(-1), { path: '/pages/owned-page/webrtc', method: 'POST', body });
  }
  for (const code of ['unrecognized', ['display_attach_busy'], null, '__proto__']) {
    guestResult = { code, error: 'ordinary guest failure' };
    const response = await fetch(endpoint, { method: 'POST', body: JSON.stringify(body) });
    assert.equal(response.status, 404);
    assert.deepEqual(await response.json(), { error: 'ordinary guest failure' });
  }
  guestStatus = 200; guestResult = { schema: 'elastos.browser.display-attach-result/v1', request_id: body.request_id };
  const success = await fetch(endpoint, { method: 'POST', body: JSON.stringify(body) });
  assert.equal(success.status, 200); assert.deepEqual(await success.json(), guestResult);
  const dispatched = guestRequests.length;
  const absent = await fetch(endpoint.replace('owned-page', 'missing-page'), { method: 'POST', body: '{}' });
  assert.equal(absent.status, 404); assert.equal(guestRequests.length, dispatched);
});
