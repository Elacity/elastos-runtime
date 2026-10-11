import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import test from 'node:test';
import { cleanupAndExit } from './browser-vm-local-crosvm-launcher.mjs';

// Like a real child: still running after SIGTERM, exits on SIGKILL.
function stubbornChild(events) {
  let onExit = () => {};
  const child = {
    exitCode: null,
    signalCode: null,
    once: (event, listener) => { if (event === 'exit') onExit = listener; },
    kill: (signal) => {
      events.push(signal);
      if (signal === 'SIGKILL') { child.signalCode = signal; onExit(); }
    },
  };
  return child;
}

function session(events, overrides = {}) {
  return { exiting: false, profileShutdownCommitted: false, launchSucceeded: true,
    guestControlSocket: null, deferredRootfsPoolRefill: null, sessionDir: null,
    children: new Set([stubbornChild(events)]), servers: new Set(), cleanupFns: [],
    privateSessionInputs: new Map(), ...overrides };
}

test('Runtime shutdown flushes the guest profile before terminating crosvm and its bridges', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'browser-shutdown-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const privateInput = path.join(directory, 'boot-params');
  fs.writeFileSync(privateInput, 'fixture secret', { mode: 0o600 });
  const { dev, ino } = fs.statSync(privateInput);
  const events = [];
  const owner = session(events, {
    guestControlSocket: '/fixture/control.sock',
    servers: new Set([{ server: { close: () => events.push('bridge closed') } }]),
    privateSessionInputs: new Map([[privateInput, { dev, ino }]]),
    cleanupFns: [() => { assert.equal(fs.existsSync(privateInput), false); events.push('lock released'); }],
  });
  const hooks = {
    shutdownGuest: async (_socket, route, options) => {
      assert.equal(route, '/shutdown');
      assert.equal(options.method, 'POST');
      events.push('profile flushed');
      return { ok: true, profile_disk_flushed: true, profile_disk_unmounted: true };
    },
    exit: (code) => { assert.equal(code, 0); events.push('exit'); },
    env: {}, sleep: async () => {}, log: (text) => { throw new Error(text); },
  };
  await cleanupAndExit('SIGTERM', owner, hooks);
  assert.deepEqual(events, ['profile flushed', 'bridge closed', 'SIGTERM', 'SIGKILL', 'lock released', 'exit']);
  await cleanupAndExit('SIGTERM', owner, hooks);
  assert.equal(events.length, 6, 'repeated signal must share one shutdown');
});

test('launcher awaits a slow pool refill after VM teardown, outside the killed children', async (t) => {
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'browser-refill-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  fs.mkdirSync(path.join(directory, 'scripts'));
  const refillScript = path.join(directory, 'scripts/browser-vm-prepare-rootfs-pool.mjs');
  const rootfs = path.join(directory, 'rootfs.ext4');
  const marker = path.join(directory, 'ready');
  fs.writeFileSync(rootfs, 'fixture');
  fs.writeFileSync(refillScript, `import fs from 'node:fs'; setTimeout(() => fs.writeFileSync(${JSON.stringify(marker)}, 'ready'), 800);`);
  const events = [];
  const owner = session(events, { deferredRootfsPoolRefill: { dataDir: directory, poolDir: directory,
    rootfs, imageSha256: 'fixture', sessionDir: directory, scriptPath: refillScript } });
  await cleanupAndExit('SIGTERM', owner, {
    env: { ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_COUNT: '2', ELASTOS_BROWSER_VM_ROOTFS_POOL_REFILL_MIN_FREE_MIB: '0' },
    sleep: async () => {}, log: () => {}, exit: () => events.push('exit'),
  });
  assert.equal(fs.readFileSync(marker, 'utf8'), 'ready');
  assert.deepEqual(events, ['SIGTERM', 'SIGKILL', 'exit']);
});

test('failed profile flush reports failure after reaping crosvm and releasing its lock', async () => {
  const events = [];
  const owner = session(events, { guestControlSocket: '/fixture/control.sock',
    cleanupFns: [() => events.push('lock released')] });
  await cleanupAndExit('SIGTERM', owner, {
    shutdownGuest: async () => ({ ok: true, profile_disk_flushed: true, profile_disk_unmounted: false }),
    env: {}, sleep: async () => {}, log: () => events.push('flush failed'),
    exit: (code) => { assert.equal(code, 1); events.push('failed exit'); },
  });
  assert.deepEqual(events, ['flush failed', 'SIGTERM', 'SIGKILL', 'lock released', 'failed exit']);
});
