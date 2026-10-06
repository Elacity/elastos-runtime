import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('./browser-vm-local-crosvm-launcher.mjs', import.meta.url), 'utf8');
const cleanup = source.slice(source.indexOf('async function cleanupAndExit('), source.indexOf('\nasync function main()'));

test('Runtime shutdown flushes the guest profile before terminating crosvm and its bridges', async () => {
  const events = [];
  const context = vm.createContext({
    exiting: false,
    SESSION_KEEP_ENV: "KEEP",
    guestControlSocket: '/fixture/control.sock',
    children: new Set([{ kill: (signal) => events.push(signal) }]),
    servers: new Set([{ server: { close: () => events.push('bridge closed') } }]),
    cleanupFns: [],
    launchSucceeded: true,
    httpJsonUnix: async (_socket, route, options) => {
      assert.equal(route, '/shutdown');
      assert.equal(options.method, 'POST');
      events.push('profile flushed');
      return { ok: true, profile_disk_flushed: true, profile_disk_unmounted: true };
    },
    process: { env: {}, exit: () => {} },
    globalThis: {},
    setTimeout: (callback) => callback(),
    logPhase: (text) => { throw new Error(text); },
  });
  vm.runInContext(cleanup, context);
  await context.cleanupAndExit('SIGTERM');
  assert.deepEqual(events, ['profile flushed', 'bridge closed', 'SIGTERM', 'SIGKILL']);
  await context.cleanupAndExit('SIGTERM');
  assert.equal(events.length, 4, 'repeated signal must share one shutdown');
});
