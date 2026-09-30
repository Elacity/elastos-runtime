import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import test from "node:test";
import vm from "node:vm";

const source = fs.readFileSync(new URL("./browser-selkies-control-service.mjs", import.meta.url), "utf8");
const names = [
  "browser-vm-initrd.log", "browser-vm-rootfs-entry.log", "browser-vm-init.log",
  "browser-vm-selkies-control.log", "browser-vm-xvfb.log", "browser-vm-native-proxy.log",
  "browser-vm-chromium.log", "browser-vm-selkies.log", "browser-vm-pipewire.log",
  "browser-vm-wireplumber.log", "browser-vm-wireplumber-config.log",
  "browser-vm-pipewire-pulse.log", "browser-vm-pipewire-null-sink.log",
  "browser-vm-pipewire-summary.log", "browser-vm-pipewire-dump.log",
];

function fixture(t, overrides = {}) {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "browser-passive-logs-"));
  t.after(() => fs.rmSync(dir, { recursive: true, force: true }));
  const calls = { children: [], writes: [], reads: [], closes: [] };
  const context = vm.createContext({
    Buffer, Error, process: { env: { XDG_RUNTIME_DIR: dir } }, VM_LOG_DIR: dir,
    // Retain the old helpers in the extracted slice for a safe negative control:
    // attempted commands are counted, and never run on the host.
    execFileSync: (...args) => { calls.children.push(args); return "[]"; },
    fs: {
      ...fs,
      writeFileSync: (...args) => { calls.writes.push(args); return fs.writeFileSync(...args); },
      readSync: (...args) => { calls.reads.push(args); return fs.readSync(...args); },
      closeSync: (...args) => { calls.closes.push(args); return fs.closeSync(...args); },
      ...overrides,
    },
  });
  const constants = source.slice(source.indexOf("const VM_LOG_NAMES ="), source.indexOf("const MAX_WEBSOCKET_FRAME_BYTES"));
  const reader = source.slice(source.indexOf("function readTail("), source.indexOf("function nowIso("));
  assert.ok(constants && reader, "extract the actual log reader and its allowlist");
  vm.runInContext(`${constants}\n${reader}`, context);
  return { dir, calls, read: () => JSON.parse(JSON.stringify(context.readBrowserVmLogTails())) };
}

test("missing audio logs remain absent across repeated reads with zero child commands or writes", t => {
  const f = fixture(t);
  for (let i = 0; i < 3; i++) {
    const logs = f.read();
    assert.deepEqual(Object.keys(logs), names);
    for (const name of names) assert.deepEqual(logs[name], { present: false }, name);
  }
  assert.deepEqual(fs.readdirSync(f.dir), []);
  assert.deepEqual(f.calls.children, []);
  assert.deepEqual(f.calls.writes, []);
});

test("startup audio summary and dump retain their bytes and original modification times", t => {
  const f = fixture(t), before = new Map();
  for (const name of names) {
    const file = path.join(f.dir, name);
    fs.writeFileSync(file, `retained startup evidence: ${name}\n`);
    fs.utimesSync(file, new Date("2026-01-01T01:02:03Z"), new Date("2026-01-01T01:02:03Z"));
    before.set(name, { bytes: fs.readFileSync(file), stat: fs.statSync(file) });
  }
  const first = f.read();
  for (let i = 0; i < 3; i++) assert.deepEqual(f.read(), first);
  for (const [name, { bytes, stat }] of before) {
    const file = path.join(f.dir, name);
    assert.deepEqual(fs.readFileSync(file), bytes);
    assert.equal(fs.statSync(file).mtimeMs, stat.mtimeMs);
    assert.equal(fs.statSync(file).ctimeMs, stat.ctimeMs);
    assert.deepEqual(first[name], {
      present: true, bytes: bytes.length, mtime: stat.mtime.toISOString(), tail: bytes.toString("utf8"),
    });
  }
  assert.deepEqual(f.calls.children, []);
  assert.deepEqual(f.calls.writes, []);
});

test("each allowlisted tail reads at most 8 KiB and omits unrelated files", t => {
  const f = fixture(t);
  for (const [index, name] of names.entries()) {
    fs.writeFileSync(path.join(f.dir, name), "x".repeat(index * 2048));
  }
  fs.writeFileSync(path.join(f.dir, "unrelated-private-file"), "excluded");
  const logs = f.read();
  assert.deepEqual(Object.keys(logs), names);
  for (const [index, name] of names.entries()) {
    assert.equal(logs[name].bytes, index * 2048);
    assert.equal(logs[name].tail, "x".repeat(Math.min(index * 2048, 8192)));
  }
  assert.equal(f.calls.reads.length, names.length);
  for (const [, , , length] of f.calls.reads) assert.ok(length <= 8192);
  assert.equal(f.calls.closes.length, names.length);
  assert.deepEqual(f.calls.children, []);
});

test("the tail contains the final bytes, including an empty present log", t => {
  const f = fixture(t), body = "old".repeat(5000) + "\nlast retained event\n";
  fs.writeFileSync(path.join(f.dir, names[0]), body);
  fs.writeFileSync(path.join(f.dir, names[1]), "");
  const logs = f.read();
  assert.equal(logs[names[0]].tail, Buffer.from(body).subarray(-8192).toString("utf8"));
  assert.equal(logs[names[1]].present, true);
  assert.equal(logs[names[1]].bytes, 0);
  assert.equal(logs[names[1]].tail, "");
});

test("an unreadable log retains missing/error semantics and closes its descriptor", t => {
  const f = fixture(t, { readSync() { throw new Error("fixture read denied"); } });
  fs.writeFileSync(path.join(f.dir, names[0]), "existing");
  const logs = f.read();
  assert.deepEqual(logs[names[0]], { present: false, error: "fixture read denied" });
  assert.equal(f.calls.closes.length, 1);
  assert.deepEqual(logs[names[1]], { present: false });
});
