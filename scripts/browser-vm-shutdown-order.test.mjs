import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import vm from 'node:vm';

const source = fs.readFileSync(new URL('./browser-vm-local-crosvm-launcher.mjs', import.meta.url), 'utf8');
const cleanup = source.slice(source.indexOf('async function cleanupAndExit('), source.indexOf('\nasync function main()'));

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

test('Runtime shutdown flushes the guest profile before terminating crosvm and its bridges', async () => {
  const events = [];
  const context = vm.createContext({
    exiting: false,
    deferredRootfsPoolRefill: null,
    SESSION_KEEP_ENV: "KEEP",
    guestControlSocket: '/fixture/control.sock',
    children: new Set([stubbornChild(events)]),
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

test('launcher awaits a slow pool refill after VM teardown, outside the killed children', async (t) => {
  const { spawn } = await import('node:child_process');
  const os = await import('node:os');
  const path = await import('node:path');
  const directory = fs.mkdtempSync(path.join(os.tmpdir(), 'browser-refill-'));
  t.after(() => fs.rmSync(directory, { recursive: true, force: true }));
  const refillScript = path.join(directory, 'refill.mjs');
  const marker = path.join(directory, 'ready');
  fs.writeFileSync(refillScript, `import fs from 'node:fs'; setTimeout(() => fs.writeFileSync(${JSON.stringify(marker)}, 'ready'), 800);`);
  const events = [];
  const refillStart = source.indexOf('function maybeRefillPreparedRootfsPool(');
  const refill = source.slice(source.lastIndexOf('\n', refillStart) + 1,
                              source.indexOf('\nfunction refillPreparedRootfsPoolSync('));
  const context = vm.createContext({
    exiting: false, deferredRootfsPoolRefill: { dataDir: directory, poolDir: directory,
      rootfs: refillScript, sessionDir: directory, scriptPath: refillScript },
    ROOTFS_POOL_REFILL_COUNT_ENV: 'COUNT', SESSION_KEEP_ENV: 'KEEP',
    guestControlSocket: null, servers: new Set(), cleanupFns: [], launchSucceeded: true,
    children: new Set([stubbornChild(events)]),
    readyRootfsFiles: () => [], rootfsPoolRefillScript: () => refillScript,
    availableBytesForPath: () => null, refillMinFreeBytes: () => 0,
    rootfsPoolRefillCommand: () => ({ command: process.execPath, args: [refillScript] }),
    spawnTracked: (command, args, options) => {
      const child = spawn(command, args, options); context.children.add(child); return child;
    },
    spawn, fs, path, process: { env: {}, exit: () => events.push('exit') }, globalThis: {},
    setTimeout, clearTimeout, logPhase: () => {},
  });
  t.after(() => {
    for (const child of context.children) if (child.pid) child.kill('SIGKILL');
  });
  vm.runInContext(refill + '\n' + cleanup, context);
  await context.cleanupAndExit('SIGTERM');
  assert.equal(fs.readFileSync(marker, 'utf8'), 'ready');
  assert.deepEqual(events, ['SIGTERM', 'SIGKILL', 'exit']);
});
