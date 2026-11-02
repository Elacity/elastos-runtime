import assert from 'node:assert/strict';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
const helper = fileURLToPath(new URL('./browser-image-cid-input.py', import.meta.url));
const cid = 'bafkreiaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa';
const digest = 'ab'.repeat(32);
const run = (...args) => spawnSync('python3', [helper, ...args], { encoding: 'utf8', timeout: 5000 });
test('CID release inputs admit canonical identity and bounded archive bytes', () => {
  const result = run(cid, digest, '1');
  assert.equal(result.status, 0, result.stderr);
  for (const args of [[cid.toUpperCase(), digest, '1'], ['../archive', digest, '1'],
    ['b' + 'a'.repeat(58), digest, '1'], [cid, digest.toUpperCase(), '1'], [cid, digest, '0'],
    [cid, digest, String(16 * 1024 ** 3 + 1)], [cid, digest, '01']]) {
    assert.notEqual(run(...args).status, 0, args.join(' '));
  }
});
