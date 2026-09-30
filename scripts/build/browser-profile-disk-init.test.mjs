import assert from 'node:assert/strict';
import { mkdtempSync, readFileSync, writeFileSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const sourcePath = process.env.B12_PROFILE_STAGE_SOURCE || fileURLToPath(new URL('./stage-browser-vm-target.sh', import.meta.url));
const source = readFileSync(sourcePath, 'utf8');
const start = source.indexOf('mount_browser_profile_disk() {\n');
const end = source.indexOf('\ncase "$profile_key" in', start);
assert.ok(start > 0 && end > start, 'actual generated guest mount function required');
const mountFunction = source.slice(start, end)
  .replace('disk="/dev/vdb"', 'disk="$B12_DISK"')
  .replace('mount_dir="/var/lib/elastos/browser-profile-disk"', 'mount_dir="$B12_MOUNT_DIR"');
const key = `profile-${'a'.repeat(64)}`;
const marker = `ELASTOS_BROWSER_PROFILE_NEW_V1:${key}`;
const newDisk = () => Buffer.concat([Buffer.from(marker), Buffer.alloc(8192 - marker.length)]);

function runCase({ bytes = Buffer.from('existing profile bytes: cookies, localStorage, IndexedDB'), intent = '', mount = 'fail', formatter = 'ok', consume = 'ok', sync = 'ok', present = '1', repeat = false } = {}) {
  const root = mkdtempSync(join(tmpdir(), 'b12-profile-init-'));
  try {
    const disk = join(root, 'disk');
    const events = join(root, 'events');
    writeFileSync(disk, bytes);
    writeFileSync(events, '');
    // Execute the actual generated POSIX function. Only fixed guest paths and
    // the block-device predicate are adapted for regular, small local files.
    // Mount/formatter/sync are shell fixtures; real dd consumes marker bytes.
    const harness = `set -eu
function [ {
  if [[ $# == 3 && $1 == -b ]]; then
    [[ $2 == "$B12_DISK" && "$B12_PRESENT" == 1 ]]
  elif [[ $# == 4 && $1 == ! && $2 == -b ]]; then
    [[ $3 != "$B12_DISK" || "$B12_PRESENT" != 1 ]]
  else builtin [ "$@"; fi
}
sleep() { :; }
grep() { return 1; }
mount() {
  printf 'mount\\n' >> "$B12_EVENTS"
  [[ "$B12_MOUNT" == success ]] && return 0
  if [[ "$B12_MOUNT" == after_format ]] && [[ "$(head -c 4 "$B12_DISK")" == EXT4 ]]; then return 0; fi
  return 1
}
mke2fs() {
  printf 'format\\n' >> "$B12_EVENTS"
  [[ "$B12_FORMATTER" == fail ]] && return 1
  printf 'EXT4 initialized' > "$B12_DISK"
}
sync() { printf 'sync\\n' >> "$B12_EVENTS"; [[ "$B12_SYNC" == ok ]]; }
dd() {
  if [[ "$*" == *"of=$B12_DISK"* ]]; then
    printf 'consume\\n' >> "$B12_EVENTS"
    [[ "$B12_CONSUME" == fail ]] && return 1
  fi
  command dd "$@"
}
profile_disk_initialize="$B12_INTENT"
${mountFunction}
mount_browser_profile_disk "$B12_KEY"
${repeat ? 'B12_MOUNT=fail\nmount_browser_profile_disk "$B12_KEY"' : ''}
printf 'profile=%s\\n' "$ELASTOS_BROWSER_VM_PROFILE_DIR"
`;
    const result = spawnSync('bash', ['-c', harness], { encoding: 'utf8', timeout: 5000,
      env: { ...process.env, B12_DISK: disk, B12_MOUNT_DIR: join(root, 'mount'), B12_EVENTS: events,
        B12_KEY: key, B12_INTENT: intent, B12_MOUNT: mount, B12_FORMATTER: formatter,
        B12_CONSUME: consume, B12_SYNC: sync, B12_PRESENT: present } });
    assert.ifError(result.error);
    return { ...result, bytes: readFileSync(disk), events: readFileSync(events, 'utf8').trim().split('\n').filter(Boolean) };
  } finally { rmSync(root, { recursive: true, force: true }); }
}

test('existing disk mount failure preserves all bytes and never formats', () => {
  const bytes = Buffer.from('existing profile bytes: corruption is not creation intent');
  const result = runCase({ bytes });
  assert.notEqual(result.status, 0);
  assert.equal(result.events.filter(x => x === 'format').length, 0, 'existing disk must receive zero formatter calls');
  assert.deepEqual(result.bytes, bytes);
  assert.match(result.stderr, /preserved/);
});

test('missing filesystem signature without host intent preserves zero-filled disk', () => {
  const bytes = Buffer.alloc(8192);
  const result = runCase({ bytes });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount']);
  assert.deepEqual(result.bytes, bytes);
});

test('valid existing ext4 mount succeeds without formatting even with stale boot intent', () => {
  const bytes = Buffer.from('existing ext4 fixture');
  const result = runCase({ bytes, intent: 'new', mount: 'success' });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.events, ['mount']);
  assert.deepEqual(result.bytes, bytes);
  assert.match(result.stdout, /profile=.*\/profiles\/profile-a+/);
});

test('host intent plus matching new-disk marker initializes once then mounts', () => {
  const result = runCase({ bytes: newDisk(), intent: 'new', mount: 'after_format' });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(result.events, ['mount', 'consume', 'sync', 'format', 'mount']);
  assert.equal(result.bytes.toString(), 'EXT4 initialized');
});

test('host intent alone cannot format unknown existing bytes', () => {
  const bytes = Buffer.from('unknown existing disk');
  const result = runCase({ bytes, intent: 'new' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount']);
  assert.deepEqual(result.bytes, bytes);
});

test('new-disk marker alone cannot format without current host intent', () => {
  const bytes = newDisk();
  const result = runCase({ bytes });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount']);
  assert.deepEqual(result.bytes, bytes);
});

test('another profile marker cannot authorize initialization', () => {
  const bytes = newDisk(); bytes[marker.length - 1] = 'b'.charCodeAt(0);
  const result = runCase({ bytes, intent: 'new' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount']);
  assert.deepEqual(result.bytes, bytes);
});

test('consumption failure stops before formatter', () => {
  const bytes = newDisk();
  const result = runCase({ bytes, intent: 'new', consume: 'fail' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount', 'consume']);
  assert.deepEqual(result.bytes, bytes);
});

test('formatter failure consumes initialization intent and permits no retry format', () => {
  const first = runCase({ bytes: newDisk(), intent: 'new', formatter: 'fail' });
  assert.notEqual(first.status, 0);
  assert.deepEqual(first.events, ['mount', 'consume', 'sync', 'format']);
  assert.deepEqual(first.bytes, Buffer.alloc(8192));
  const retry = runCase({ bytes: first.bytes, intent: 'new' });
  assert.notEqual(retry.status, 0);
  assert.deepEqual(retry.events, ['mount']);
  assert.deepEqual(retry.bytes, first.bytes);
});

test('same boot intent cannot format a profile after successful first initialization', () => {
  const result = runCase({ bytes: newDisk(), intent: 'new', mount: 'after_format', repeat: true });
  assert.notEqual(result.status, 0);
  assert.equal(result.events.filter(x => x === 'format').length, 1);
  assert.equal(result.bytes.toString(), 'EXT4 initialized');
});

test('missing device fails without mount or formatting', () => {
  const bytes = newDisk();
  const result = runCase({ bytes, intent: 'new', present: '0' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, []);
  assert.deepEqual(result.bytes, bytes);
});

test('Linux crosvm directory profile first-run keeps its existing guest path', () => {
  const root = mkdtempSync(join(tmpdir(), 'b12-crosvm-profile-'));
  try {
    const stop = source.indexOf('\nselkies_checkpoint "profile initialized"', end);
    assert.ok(stop > end);
    const dispatch = source.slice(end, stop).replace('"/var/lib/elastos/browser-profiles/$profile_key"', '"$B12_PROFILES/$profile_key"');
    const result = spawnSync('bash', ['-c', `set -eu\nprofile_key=principal-fixture\nprofile_disk_policy=\nmount_browser_profile_disk() { echo unexpected-disk-mount >&2; exit 91; }\n${dispatch}\nprintf '%s' "$ELASTOS_BROWSER_VM_PROFILE_DIR"`],
      { encoding: 'utf8', timeout: 1000, env: { ...process.env, B12_PROFILES: root } });
    assert.ifError(result.error);
    assert.equal(result.status, 0, result.stderr);
    assert.equal(result.stdout, join(root, 'principal-fixture'));
  } finally { rmSync(root, { recursive: true, force: true }); }
});


test('failed intent flush stops before format and preserves the remaining new disk', () => {
  const result = runCase({ bytes: newDisk(), intent: 'new', sync: 'fail' });
  assert.notEqual(result.status, 0);
  assert.deepEqual(result.events, ['mount', 'consume', 'sync']);
  assert.deepEqual(result.bytes, Buffer.alloc(8192));
});

test('failed mount after initialization cannot trigger a second format', () => {
  const first = runCase({ bytes: newDisk(), intent: 'new' });
  assert.notEqual(first.status, 0);
  assert.deepEqual(first.events, ['mount', 'consume', 'sync', 'format', 'mount']);
  const retry = runCase({ bytes: first.bytes, intent: 'new' });
  assert.notEqual(retry.status, 0);
  assert.deepEqual(retry.events, ['mount']);
  assert.deepEqual(retry.bytes, first.bytes);
});
