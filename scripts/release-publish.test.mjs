import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

const source = dirname(fileURLToPath(import.meta.url));
const platforms = ['aarch64-darwin', 'x86_64-linux', 'aarch64-linux'];
const commit = 'a'.repeat(40);
const tree = 'b'.repeat(40);
const ciRun = { id: 456, html_url: 'https://github.com/Elacity/elastos-runtime/actions/runs/456',
  head_sha: commit, head_branch: 'develop', event: 'push', path: '.github/workflows/ci.yml',
  status: 'completed', conclusion: 'success' };
const requiredJobs = ['test-elastos', 'source-home-macos'];
const sha = bytes => createHash('sha256').update(bytes).digest('hex');

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', ...options });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result;
}

function fixture(t, changes = {}) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'release-publish-fixture-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repo = join(root, 'repo');
  const bin = join(root, 'bin');
  mkdirSync(join(repo, 'scripts'), { recursive: true });
  mkdirSync(bin);
  cpSync(join(source, 'release-publish.sh'), join(repo, 'scripts/release-publish.sh'));
  const config = { root, repo, commit, tree, artifacts: {}, ciRuns: [ciRun], requiredJobs,
    ciJobs: requiredJobs.map(name => ({ name, status: 'completed', conclusion: 'success' })), ...changes };
  for (const [index, platform] of platforms.entries()) {
    const input = join(root, platform, 'inputs');
    mkdirSync(input, { recursive: true });
    for (const [name, version] of [['N', '1.2.3'], ['N1', '1.2.4']]) {
      const dir = join(input, name);
      mkdirSync(join(dir, 'artifacts'), { recursive: true });
      const receipt = { platform, version, source: { commit, tree }, ...changes[platform] };
      writeFileSync(join(dir, 'platform-input.json'), JSON.stringify(receipt));
      writeFileSync(join(dir, 'artifacts', `elastos-${platform}`), `${platform}-${version}`);
      writeFileSync(join(dir, 'artifacts', 'shared.tar.gz'), 'shared bytes');
      if (platform === 'aarch64-darwin') {
        const kubo = join(root, 'kubo');
        mkdirSync(kubo, { recursive: true });
        writeFileSync(join(kubo, 'ipfs'), 'fixture Kubo');
        run('tar', ['-czf', join(dir, 'artifacts/kubo-darwin-arm64.tar.gz'), '-C', root, 'kubo']);
      }
    }
    run('bash', ['-c', 'find . -type f -print0 | sort -z | xargs -0 shasum -a 256 > ../SHA256SUMS; mv ../SHA256SUMS .'], { cwd: input });
    const parts = join(root, platform, 'parts');
    mkdirSync(parts);
    run('tar', ['-czf', join(parts, 'release-inputs.tar.gz.partaaa'), '-C', join(root, platform), 'inputs']);
    const part = readFileSync(join(parts, 'release-inputs.tar.gz.partaaa'));
    writeFileSync(join(parts, 'SHA256SUMS'), `${sha(part)}  release-inputs.tar.gz.partaaa\n`);
    const zip = join(root, `${platform}.zip`);
    run('zip', ['-q', zip, 'SHA256SUMS', 'release-inputs.tar.gz.partaaa'], { cwd: parts });
    const prefix = platform === 'aarch64-darwin' ? 'release-mac' : `release-${platform}`;
    config.artifacts[prefix] = { id: String(index + 1), digest: sha(readFileSync(zip)), zip };
  }
  if (changes.badDigest) config.artifacts['release-aarch64-linux'].digest = '0'.repeat(64);
  writeFileSync(join(root, 'config.json'), JSON.stringify(config));
  const stub = `#!/usr/bin/env node
const fs = require('node:fs');
const path = require('node:path');
const c = JSON.parse(fs.readFileSync(process.env.FIXTURE_CONFIG));
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(path.join(c.root, 'calls.jsonl'), JSON.stringify({ command, args }) + '\\n');
if (command === 'gh') {
  const endpoint = args[1];
  if (c.failCiApi && endpoint.includes(c.failCiApi)) process.exit(94);
  let payload;
  if (endpoint.includes('/workflows/ci.yml/runs?')) {
    if (!endpoint.includes('head_sha=' + c.commit) || !endpoint.includes('branch=develop') || !endpoint.includes('event=push')) process.exit(93);
    payload = { workflow_runs: c.ciRuns };
  } else if (endpoint.endsWith('/branches/develop/protection')) payload = { required_status_checks: { contexts: c.requiredJobs } };
  else if (endpoint.includes('/runs/456/jobs?')) payload = { jobs: c.ciJobs };
  if (payload) {
    if (!args.includes('--jq')) { console.log(JSON.stringify(payload)); process.exit(0); }
    const result = require('node:child_process').spawnSync('jq', ['-r', args[args.indexOf('--jq') + 1]], { input: JSON.stringify(payload), encoding: 'utf8' });
    process.stdout.write(result.stdout); process.stderr.write(result.stderr); process.exit(result.status);
  } else if (endpoint.endsWith('/zip')) {
    const id = endpoint.split('/').at(-2);
    process.stdout.write(fs.readFileSync(Object.values(c.artifacts).find(a => a.id === id).zip));
  } else if (endpoint.includes('/artifacts?')) {
    const prefix = Object.keys(c.artifacts).find(p => args.at(-1).includes('^' + p + '-'));
    const a = c.artifacts[prefix];
    console.log(a.id + ' ' + a.digest + ' ' + prefix + '-' + c.commit + '-1');
  } else if (endpoint.includes('/compare/')) console.log('ahead');
  else if (endpoint.endsWith('/git/ref/heads/develop')) console.log(c.commit);
  else console.log('.github/workflows/release-package.yml success');
} else if (command === 'git') {
  if (args.includes('add')) fs.cpSync(c.repo, args[args.indexOf('add') + 3], { recursive: true });
  else if (args.includes('rev-parse')) console.log(c.tree);
  else if (!args.includes('fetch')) process.exit(91);
} else if (command === 'curl') {
  const url = args.at(-1);
  if (url.endsWith('/zip')) {
    const id = url.split('/').at(-2);
    fs.copyFileSync(Object.values(c.artifacts).find(a => a.id === id).zip, args[args.indexOf('-o') + 1]);
  } else console.log(JSON.stringify(url.includes('bootstrap') ? { ticket: 'public-ticket', node_id: 'public-node' } : { signer_did: 'did:fixture' }));
} else if (command === 'scp') fs.writeFileSync(args.at(-1), '{}');
else process.exit(92);
`;
  for (const name of ['gh', 'git', 'curl', 'scp']) writeFileSync(join(bin, name), stub, { mode: 0o755 });
  writeFileSync(join(bin, 'sha256sum'), '#!/bin/sh\nexec shasum -a 256 "$@"\n', { mode: 0o755 });
  writeFileSync(join(repo, 'scripts/release-platform-input.py'), `import json, os, pathlib, sys
p = pathlib.Path(sys.argv[2])
receipt = json.loads((p / 'platform-input.json').read_text())
with open(os.environ['VERIFY_LOG'], 'a') as log:
    log.write(json.dumps(receipt) + '\\n')
if os.environ.get('FAIL_VERIFY') == receipt['platform']:
    raise SystemExit('fixture verification failure')
`);
  writeFileSync(join(repo, 'scripts/publish-release.sh'), `#!/usr/bin/env node
const fs = require('node:fs');
const args = process.argv.slice(2);
fs.writeFileSync(process.env.ARGUMENT_LOG, JSON.stringify(args));
const output = args[args.indexOf('--prepare-only') + 1];
fs.mkdirSync(output, { recursive: true });
fs.writeFileSync(output + '/signing-input.json', JSON.stringify({ source: { commit: '${commit}', tree: '${tree}' }, files: [{ size: 1 }], installer: { stamps: { MAINTAINER_DID: 'did:fixture' } } }));
`, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, FIXTURE_CONFIG: join(root, 'config.json'),
    RELEASE_WORK: join(root, 'work'), RELEASE_ORIGIN: 'https://fixture.invalid',
    RELEASE_SEED: 'fixture.invalid', RELEASE_SEED_DATA: '/fixture/data',
    RELEASE_SEED_UNIT: 'fixture.service', RELEASE_SEED_STAGE: join(root, 'stage'),
    RELEASE_SEED_RUNTIME: '/fixture/elastos', VERIFY_LOG: join(root, 'verify.jsonl'),
    ARGUMENT_LOG: join(root, 'arguments.json') };
  return { root, env, script: join(repo, 'scripts/release-publish.sh'),
    prepare(version = '1.2.3', selected = [], extraEnv = {}) {
      return spawnSync('bash', [this.script, 'prepare', '123', version, ...selected],
        { encoding: 'utf8', env: { ...env, ...extraEnv } });
    },
    args() { return JSON.parse(readFileSync(env.ARGUMENT_LOG)); } };
}

for (const selected of [[], ['aarch64-darwin', 'x86_64-linux'], ['aarch64-darwin']]) {
  test(`prepare passes each platform once: ${selected.join(',') || 'all'}`, t => {
    const f = fixture(t);
    const result = f.prepare('1.2.4', selected);
    assert.equal(result.status, 0, result.stderr);
    const expected = selected.length ? selected : platforms;
    const args = f.args();
    const inputs = args.flatMap((arg, i) => arg === '--platform-input' ? [args[i + 1]] : []);
    assert.deepEqual(inputs, expected.map(p => `${p}=${f.root}/work/1.2.4/inputs/${p}/N1`));
    assert.equal(args.includes('--preview-platform'), expected.length === 1);
    assert.equal(readFileSync(f.env.VERIFY_LOG, 'utf8').trim().split('\n').length, expected.length);
    const record = JSON.parse(readFileSync(join(f.root, 'work/1.2.4/run.json')));
    assert.deepEqual(record.ci_run, { id: ciRun.id, url: ciRun.html_url });
    assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
  });
}

for (const [label, changes, error] of [
  ['failed CI', { ciRuns: [{ ...ciRun, conclusion: 'failure' }] }, /successful develop push CI run/],
  ['no CI run', { ciRuns: [] }, /successful develop push CI run/],
  ['skipped required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'skipped' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: skipped/],
  ['failed required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'failure' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: failure/],
  ['missing required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'success' }] }, /source-home-macos: missing/],
  ['wrong source CI', { ciRuns: [{ ...ciRun, head_sha: 'c'.repeat(40) }] }, /successful develop push CI run/],
  ['PR CI', { ciRuns: [{ ...ciRun, event: 'pull_request' }] }, /successful develop push CI run/],
  ['wrong branch CI', { ciRuns: [{ ...ciRun, head_branch: 'main' }] }, /successful develop push CI run/],
  ['incomplete CI', { ciRuns: [{ ...ciRun, status: 'in_progress' }] }, /successful develop push CI run/],
  ['unreadable branch protection', { failCiApi: '/branches/develop/protection' }, /cannot read develop required jobs/],
  ['unreadable CI jobs', { failCiApi: '/runs/456/jobs?' }, /cannot read develop CI jobs/],
]) {
  test(`prepare refuses ${label} before unsigned preparation`, t => {
    const f = fixture(t, changes);
    const result = f.prepare();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.throws(() => f.args(), { code: 'ENOENT' });
    const calls = readFileSync(join(f.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(calls.some(c => c.command === 'git' || c.command === 'scp'), false);
  });
}

test('policy prints the prepared source, package run and develop CI run', t => {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const signer = join(f.root, 'signer.py');
  const key = join(f.root, 'fixture-key');
  writeFileSync(signer, '# fixture signer');
  writeFileSync(key, 'fixture key');
  const openssl = run('which', ['openssl']).stdout.trim();
  const result = run('bash', [f.script, 'policy', '1.2.3', signer, key, openssl], { env: f.env });
  assert.match(result.stdout, new RegExp(`Source commit: ${commit}`));
  assert.match(result.stdout, /Package run: 123 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/123/);
  assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
});

for (const selected of [['x86_64-linux'], ['aarch64-darwin', 'aarch64-darwin'], ['unknown']]) {
  test(`prepare refuses invalid platform selection: ${selected.join(',')}`, t => {
    const f = fixture(t);
    assert.notEqual(f.prepare('1.2.3', selected).status, 0);
  });
}

for (const [label, changes, extraEnv] of [
  ['different source', { 'aarch64-linux': { source: { commit: 'c'.repeat(40), tree } } }, {}],
  ['different tree', { 'aarch64-linux': { source: { commit, tree: 'c'.repeat(40) } } }, {}],
  ['different version', { 'aarch64-linux': { version: '9.9.9' } }, {}],
  ['different platform', { 'aarch64-linux': { platform: 'x86_64-linux' } }, {}],
  ['wrong artifact digest', { badDigest: true }, {}],
  ['failed receipt verification', {}, { FAIL_VERIFY: 'aarch64-linux' }],
]) {
  test(`prepare refuses ${label} before unsigned preparation`, t => {
    const f = fixture(t, changes);
    assert.notEqual(f.prepare('1.2.3', [], extraEnv).status, 0);
    assert.throws(() => f.args(), { code: 'ENOENT' });
  });
}

test('seed reconstructs all platform files and copies every other signed file from the Mac', t => {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const signed = join(f.root, 'signed');
  mkdirSync(signed);
  writeFileSync(join(signed, 'release-head.json'), JSON.stringify({ signer_did: 'did:fixture' }));
  writeFileSync(join(signed, 'install.sh'), 'signed installer');
  writeFileSync(join(signed, 'release.json'), 'signed release');
  for (const p of platforms) {
    writeFileSync(join(signed, `components-${p}.json`), `signed components ${p}`);
    cpSync(join(f.root, p, 'inputs/N/artifacts', `elastos-${p}`), join(signed, `elastos-${p}`));
  }
  cpSync(join(f.root, 'aarch64-darwin/inputs/N/artifacts/shared.tar.gz'), join(signed, 'shared.tar.gz'));
  // A signed file no native input carries.
  writeFileSync(join(signed, 'mac-only.bin'), 'mac only');
  const result = run('bash', [f.script, 'seed', '1.2.3', signed], { env: f.env });
  assert.match(result.stdout, new RegExp(`Source commit: ${commit}`));
  assert.match(result.stdout, /Package run: 123 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/123/);
  assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
  run('bash', ['-n'], { input: result.stdout });
  const stage = join(f.root, 'stage/1.2.3');
  mkdirSync(join(stage, 'signed'), { recursive: true });
  // Copy exactly what the printed Mac scp line names.
  const copied = result.stdout.match(/^scp (.+) \S+:\S+\/signed\/$/m)[1].split(' ');
  for (const file of copied) cpSync(file, join(stage, 'signed', file.split('/').at(-1)));
  assert.ok(copied.every(file => !/elastos-|shared/.test(file)), 'native artifacts come from the run');
  cpSync(join(f.root, 'work/1.2.3/signed.SHA256SUMS'), join(stage, 'signed.SHA256SUMS'));
  // Run only artifact reconstruction. Service and publication commands stay printed.
  const reconstruction = result.stdout.split('# On the seed as the service user,')[1].split('export ELASTOS_DATA_DIR=')[0];
  run('bash', ['-c', '#' + reconstruction], { env: { ...f.env, GH_TOKEN: 'fixture' } });
  for (const p of platforms) assert.equal(readFileSync(join(stage, 'signed', `elastos-${p}`), 'utf8'), `${p}-1.2.3`);
  assert.equal(readFileSync(join(stage, 'signed/mac-only.bin'), 'utf8'), 'mac only');
});
