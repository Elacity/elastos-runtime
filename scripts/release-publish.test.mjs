import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { cpSync, existsSync, mkdirSync, mkdtempSync, readdirSync, readFileSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
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
const queueRun = { ...ciRun, event: 'merge_group', head_branch: `gh-readonly-queue/develop/pr-1-${commit}` };
const requiredJobs = ['test-elastos', 'source-home-macos'];
const sha = bytes => createHash('sha256').update(bytes).digest('hex');
const workflowCommit = 'b'.repeat(40);
const workflowTree = 'c'.repeat(40);
const did = 'did:key:z6MktwupdmLXVVqTzCw4i46r4uGyosGXRnR3XjN4Zq7oMMsw';
const nodeId = '4'.repeat(64);
const ticketBits = [...Buffer.from(JSON.stringify({ endpoints: [{ id: nodeId, addrs: [] }] }))]
  .map(byte => byte.toString(2).padStart(8, '0')).join('');
const ticket = ticketBits.match(/.{1,5}/g).map(group => 'abcdefghijklmnopqrstuvwxyz234567'[parseInt(group.padEnd(5, '0'), 2)]).join('');
const delivery = { MAINTAINER_DID: did, SOURCE_CONNECT_TICKET: ticket, PUBLISHER_GATEWAY: 'https://fixture.invalid',
  PUBLISHER_NODE_ID: nodeId, IPNS_NAME: 'fixture-ipns' };
const packageRun = { id: 123, run_attempt: 1, head_sha: workflowCommit, head_branch: 'develop', event: 'workflow_dispatch',
  path: '.github/workflows/release-package.yml', status: 'completed', conclusion: 'success',
  head_repository: { id: 1234, full_name: 'Elacity/elastos-runtime' }, repository: { id: 1234, full_name: 'Elacity/elastos-runtime' } };
// The CIDv0 of the release-head.json bytes the fixture origin serves (curl stub below).
const publicHead = 'QmZ9v8LQsW12TgsHKE9KtL5F1Vm3wtBe4BCNP9X6mEHWEh';
const updatePin = { schema: 'elastos.update-hop.previous-release/v1', version: '1.2.2', release_cid: 'Qm' + 'R'.repeat(44),
  release_sha256: '9'.repeat(64), head_cid: publicHead, signer_did: did };
const oldestPin = { ...updatePin, schema: 'elastos.update-hop.oldest-release/v1', components_schema: 'elastos.components/v1',
  component_kinds: [] };

function run(command, args, options = {}) {
  const result = spawnSync(command, args, { encoding: 'utf8', env: { ...process.env, COPYFILE_DISABLE: '1' }, ...options });
  assert.equal(result.status, 0, result.stderr || result.error?.message);
  return result;
}

function fixture(t, changes = {}) {
  // Approval/tool pins require protected ancestors; system tmp is world writable.
  const root = realpathSync(mkdtempSync(join(source, '..', 'release-publish-fixture-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const repo = join(root, 'repo');
  const bin = join(root, 'bin');
  mkdirSync(join(repo, 'scripts'), { recursive: true });
  mkdirSync(bin);
  cpSync(join(source, 'release-publish.sh'), join(repo, 'scripts/release-publish.sh'));
  cpSync(join(source, 'release-signer.py'), join(repo, 'scripts/release-signer.py'));
  cpSync(join(source, 'release-provenance.py'), join(repo, 'scripts/release-provenance.py'));
  cpSync(join(source, 'release-provenance-seal.py'), join(repo, 'scripts/release-provenance-seal.py'));
  const [installVersion, updateVersion] = changes.versions || ['1.2.3-alpha.1', '1.2.3-alpha.2'];
  mkdirSync(join(repo, 'elastos'));
  writeFileSync(join(repo, 'elastos/Cargo.toml'), `[workspace.package]\nversion = "${changes.cargoVersion || installVersion}"\n`);
  const config = { root, repo, commit, tree, artifacts: {}, ciRuns: [ciRun], requiredJobs,
    ciJobs: requiredJobs.map(name => ({ name, status: 'completed', conclusion: 'success' })),
    packageRun, workflowCommit, workflowTree, delivery, trees: {}, updatePin, oldestPin,
    tagOid: 'e'.repeat(40), mainOid: 'd'.repeat(40), ...changes };
  for (const [root, blob] of [[tree, '7'.repeat(40)], [config.workflowTree, changes.workflowBlob || '7'.repeat(40)]]) {
    const github = sha(root + '.github').slice(0, 40), workflows = sha(root + 'workflows').slice(0, 40);
    const entry = (path, type, sha) => ({ path, type, mode: type === 'tree' ? '040000' : '100644', sha });
    config.trees[root] = { sha: root, truncated: false, tree: [entry('.github', 'tree', github)] };
    config.trees[github] = { sha: github, truncated: false, tree: [entry('workflows', 'tree', workflows)] };
    config.trees[workflows] = { sha: workflows, truncated: false, tree: [entry('release-package.yml', 'blob', blob)] };
  }
  // The darwin package Runtime answers the key-free Community verifier like the real one.
  const runtime = `#!${process.execPath}
const fs = require('node:fs');
const crypto = require('node:crypto');
const c = JSON.parse(fs.readFileSync(${JSON.stringify(join(root, 'config.json'))}));
const args = process.argv.slice(2);
fs.appendFileSync(c.root + '/runtime.jsonl', JSON.stringify({ args, home: process.env.HOME }) + '\\n');
if (args.join(' ') !== 'collaboration-config verify --input ' + args[3]) process.exit(2);
if (c.runtimeRefuses) { console.error('profile signer is not trusted'); process.exit(1); }
const bytes = fs.readFileSync(args[3]);
const config = JSON.parse(bytes);
console.log(JSON.stringify({ schema: 'elastos.collaboration.operator-receipt/v1', network_id: config.expected_network_id,
  signer_did: c.runtimeSigner || config.trusted_profile_signer_dids[0],
  config_sha256: 'sha256:' + crypto.createHash('sha256').update(bytes).digest('hex') }));
`;
  for (const [index, platform] of (changes.ciOnly ? [] : platforms.entries())) {
    const input = join(root, platform, 'inputs');
    mkdirSync(input, { recursive: true });
    for (const [name, version] of [['N', installVersion], ['N1', updateVersion]]) {
      const dir = join(input, name);
      mkdirSync(join(dir, 'artifacts'), { recursive: true });
      const receipt = { schema: 'elastos.release-platform-input/v1', platform, version, source: { commit, tree, clean: true }, ...changes[platform] };
      writeFileSync(join(dir, 'platform-input.json'), JSON.stringify(receipt));
      const verifier = platform === 'aarch64-darwin' && changes.network;
      writeFileSync(join(dir, 'artifacts', `elastos-${platform}`), verifier ? runtime : `${platform}-${version}`,
        verifier ? { mode: 0o755 } : {});
      writeFileSync(join(dir, 'components.json'), JSON.stringify({ schema: 'elastos.components/v1', external: {} }));
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
    writeFileSync(join(parts, 'provenance.jsonl'), '{}');
    writeFileSync(join(parts, 'SHA256SUMS'), `${sha(part)}  release-inputs.tar.gz.partaaa\n${sha(Buffer.from('{}'))}  provenance.jsonl\n`);
    const zip = join(root, `${platform}.zip`);
    run('zip', ['-q', zip, 'SHA256SUMS', 'release-inputs.tar.gz.partaaa', 'provenance.jsonl'], { cwd: parts });
    const prefix = platform === 'aarch64-darwin' ? 'release-mac' : `release-${platform}`;
    config.artifacts[prefix] = { id: String(index + 1), digest: sha(readFileSync(zip)), zip, attempt: changes.nativeAttempt || 1 };
    proofFixture(prefix, index + 101, platform, input);
  }
  if (changes.seedFixture) {
    const parts = join(root, 'seed-package');
    mkdirSync(parts);
    const files = {};
    for (const name of ['elastos', 'ipfs-provider', 'source.tar.gz']) {
      const bytes = Buffer.from(`attested seed ${name}`);
      writeFileSync(join(parts, name), bytes);
      files[name] = { sha256: sha(bytes), size: bytes.length };
    }
    writeFileSync(join(parts, 'seed-input.json'), JSON.stringify({ schema: 'elastos.release-seed-input/v1',
      source: { commit, tree }, version: '1.2.3-alpha.1', files }));
    writeFileSync(join(parts, 'provenance.jsonl'), '{}');
    const names = ['elastos', 'ipfs-provider', 'source.tar.gz', 'seed-input.json', 'provenance.jsonl'];
    writeFileSync(join(parts, 'SHA256SUMS'), names.map(name => `${sha(readFileSync(join(parts, name)))}  ${name}\n`).join(''));
    const zip = join(root, 'seed.zip');
    run('zip', ['-q', zip, 'SHA256SUMS', ...names], { cwd: parts });
    config.artifacts['release-seed'] = { id: '901', digest: sha(readFileSync(zip)), zip, attempt: changes.nativeAttempt || 1 };
    proofFixture('release-seed', 902, 'seed', parts);
  }
  function proofFixture(prefix, id, platform, input) {
    const proof = join(root, `proof-${platform}`);
    mkdirSync(proof);
    const native = config.artifacts[prefix];
    const attempt = changes.proofAttempt || 1;
    const producer = { artifact: { id: Number(native.id), digest: 'sha256:' + native.digest,
      name: `${prefix}-${commit}-${native.attempt}`, attempt: native.attempt }, run_id: 123,
      workflow_commit: workflowCommit, attestation_attempt: attempt, ...changes.producer };
    const names = platform === 'seed' ? ['seed-input.json'] : ['N.platform-input.json', 'N1.platform-input.json'];
    for (const name of names) {
      const original = platform === 'seed' ? join(input, name) : join(input, name.split('.')[0], 'platform-input.json');
      writeFileSync(join(proof, name), JSON.stringify({ ...JSON.parse(readFileSync(original)), producer }));
    }
    writeFileSync(join(proof, 'provenance.jsonl'), '{}');
    const zip = join(root, `proof-${platform}.zip`);
    run('zip', ['-q', zip, ...names, 'provenance.jsonl'], { cwd: proof });
    config.artifacts[prefix.replace('release-', 'release-proof-')] = { id: String(id), digest: sha(readFileSync(zip)), zip, attempt };
  }
  if (changes.badDigest) config.artifacts['release-aarch64-linux'].digest = '0'.repeat(64);
  writeFileSync(join(root, 'config.json'), JSON.stringify(config));
  const stub = `#!${process.execPath}
const fs = require('node:fs');
const path = require('node:path');
const crypto = require('node:crypto');
const c = JSON.parse(fs.readFileSync(${JSON.stringify(join(root, 'config.json'))}));
const command = path.basename(process.argv[1]);
const args = process.argv.slice(2);
fs.appendFileSync(path.join(c.root, 'calls.jsonl'), JSON.stringify({ command, args }) + '\\n');
if (command === 'gh') {
  if (args[0] === 'attestation') {
    if (c.failAttestation) process.exit(95);
    const uri = 'https://github.com/Elacity/elastos-runtime/.github/workflows/release-package.yml@refs/heads/develop';
    const certificate = { issuer: 'https://token.actions.githubusercontent.com', subjectAlternativeName: uri,
      buildSignerURI: uri, buildSignerDigest: c.workflowCommit, runnerEnvironment: 'github-hosted',
      sourceRepositoryURI: 'https://github.com/Elacity/elastos-runtime', sourceRepositoryDigest: c.workflowCommit,
      sourceRepositoryIdentifier: '1234', sourceRepositoryRef: 'refs/heads/develop',
      buildConfigURI: uri, buildConfigDigest: c.workflowCommit, buildTrigger: 'workflow_dispatch',
      runInvocationURI: 'https://github.com/Elacity/elastos-runtime/actions/runs/123/attempts/' + (c.proofAttempt || 1), ...c.certificate };
    console.log(JSON.stringify([{ verificationResult: { signature: { certificate }, verifiedTimestamps: [{ type: 'Tlog' }],
      statement: { predicateType: 'https://slsa.dev/provenance/v1', subject: [{ digest: { sha256: crypto.createHash('sha256').update(fs.readFileSync(args[2])).digest('hex') } }] } } }]));
    process.exit(0);
  }
  const endpoint = args.find(arg => /^(\\/)?repos\\//.test(arg)).replace(/^\\//, '');
  if (c.failCiApi && endpoint.includes(c.failCiApi)) process.exit(94);
  let payload;
  if (endpoint.includes('/workflows/ci.yml/runs?')) {
    if (!endpoint.includes('head_sha=' + c.commit) || endpoint.includes('branch=') || endpoint.includes('event=')) process.exit(93);
    payload = { workflow_runs: c.ciRuns };
  } else if (endpoint.endsWith('/branches/develop/protection')) payload = { required_status_checks: { contexts: c.requiredJobs } };
  else if (endpoint.includes('/runs/456/jobs?')) payload = { jobs: c.ciJobs };
  else if (endpoint.endsWith('/actions/runs/123')) payload = c.packageRun;
  else if (endpoint.endsWith('/git/ref/heads/develop')) payload = { ref: 'refs/heads/develop', object: { type: 'commit', sha: c.commit } };
  else if (endpoint.endsWith('/git/ref/heads/main')) payload = { ref: 'refs/heads/main', object: { type: 'commit', sha: c.mainOid } };
  else if (endpoint.includes('/git/ref/tags/')) {
    if (c.noTag) process.exit(1);
    payload = { ref: 'refs/tags/' + endpoint.split('/').at(-1), object: { type: 'tag', sha: c.tagOid } };
  } else if (endpoint.includes('/git/tags/')) payload = { sha: c.tagOid, tag: c.tagName || 'v1.2.3', object: { type: 'commit', sha: c.tagTarget || c.commit } };
  else if (endpoint.includes('/git/trees/')) payload = c.trees[endpoint.split('/').at(-1)];
  else if (endpoint.includes('/git/commits/')) {
    const oid = endpoint.split('/').at(-1);
    payload = { sha: oid, tree: { sha: oid === c.workflowCommit ? c.workflowTree : oid === c.mainOid ? (c.mainTree || c.tree) : c.tree } };
  } else if (endpoint.includes('/runs/123/artifacts?')) payload = { artifacts: Object.entries(c.artifacts).map(([prefix,a]) => ({
    id: Number(a.id), digest: 'sha256:' + a.digest, name: prefix + '-' + c.commit + '-' + a.attempt, expired: false,
    workflow_run: { id: 123, head_sha: c.workflowCommit, head_branch: 'develop' }
  })) };
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
  } else if (endpoint.includes('/compare/')) console.log((endpoint.endsWith('...main') ? c.mainCompareStatus : c.compareStatus) || 'ahead');
  else console.log('.github/workflows/release-package.yml success');
} else if (command === 'git') {
  if (args.includes('add')) fs.cpSync(c.repo, args[args.indexOf('add') + 3], { recursive: true });
  else if (args.includes('rev-parse')) console.log(c.tree);
  else if (args.includes('show')) {
    const pin = { [c.commit + ':scripts/update-hop-previous-release.json']: c.updatePin,
      [c.commit + ':scripts/update-hop-oldest-release.json']: c.oldestPin }[args.at(-1)];
    if (!pin) process.exit(91);
    console.log(JSON.stringify(pin, null, 2));
  } else if (!args.includes('fetch') && !args.includes('prune')) process.exit(91);
} else if (command === 'curl') {
  const url = args.at(-1);
  if (url.endsWith('/zip')) {
    const id = url.split('/').at(-2);
    fs.copyFileSync(Object.values(c.artifacts).find(a => a.id === id).zip, args[args.indexOf('-o') + 1]);
  } else if (url.endsWith('/release-head.json') && c.headFile) process.stdout.write(fs.readFileSync(c.headFile));
  else console.log(JSON.stringify(url.includes('bootstrap') ? { ticket: c.delivery.SOURCE_CONNECT_TICKET, node_id: c.delivery.PUBLISHER_NODE_ID, ...c.bootstrap } : { signer_did: c.delivery.MAINTAINER_DID }));
} else if (command === 'scp') fs.writeFileSync(args.at(-1), '{}');
else process.exit(92);
`;
  for (const name of ['gh', 'git', 'curl', 'scp']) writeFileSync(join(bin, name), stub, { mode: 0o755 });
  const approval = { schema: 'elastos.release.approval/v1', repository: 'Elacity/elastos-runtime',
    source: { commit, tree }, workflow: { commit: workflowCommit, tree: workflowTree, ref: 'refs/heads/develop' }, delivery,
    gh: { path: join(bin, 'gh'), sha256: sha(readFileSync(join(bin, 'gh'))) }, ...changes.approval };
  const approvalPath = join(root, 'approval.json');
  writeFileSync(approvalPath, JSON.stringify(approval), { mode: 0o600 });
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
fs.writeFileSync(process.env.ARGUMENT_LOG + '.delivery', JSON.stringify({ gateway: process.env.ELASTOS_PUBLISHER_GATEWAY, ipns: process.env.ELASTOS_IPNS_NAME }));
const output = args[args.indexOf('--prepare-only') + 1];
fs.mkdirSync(output, { recursive: true });
const c = JSON.parse(fs.readFileSync(${JSON.stringify(join(root, 'config.json'))}));
const platforms = args.flatMap((arg, i) => arg === '--platform-input' ? [args[i + 1].split('=')[0]] : []);
const files = {};
const put = (name, bytes) => {
  fs.writeFileSync(output + '/' + name, bytes);
  files[name] = { size: bytes.length, sha256: require('node:crypto').createHash('sha256').update(bytes).digest('hex') };
};
// The approved source's components.json pin and release copy, as the package build carries them.
for (const p of platforms) put('components-' + p + '.json', Buffer.from(JSON.stringify(Object.assign(
  { schema: 'elastos.components/v1', external: {} }, c.network ? { collaboration_network: c.network.pin } : {}))));
if (c.network) put('collaboration-network-release-v1.json', Buffer.from(c.network.file));
fs.writeFileSync(output + '/signing-input.json', JSON.stringify({ source: { commit: '${commit}', tree: '${tree}' },
  version: args[args.indexOf('--version') + 1], channel: args[args.indexOf('--channel') + 1], files,
  release: { platforms: Object.fromEntries(platforms.map(p => [p, {}])) }, installer: { stamps: ${JSON.stringify(delivery)} } }));
`, { mode: 0o755 });
  const env = { ...process.env, PATH: `${bin}:${process.env.PATH}`, FIXTURE_CONFIG: join(root, 'config.json'),
    RELEASE_WORK: join(root, 'work'), RELEASE_ORIGIN: 'https://fixture.invalid',
    RELEASE_APPROVAL: approvalPath,
    RELEASE_SEED: 'fixture.invalid', RELEASE_SEED_DATA: join(root, 'xdg/elastos'),
    RELEASE_SEED_UNIT: 'fixture.service', RELEASE_SEED_STAGE: join(root, 'stage'),
    RELEASE_SEED_USER: 'fixture-user',
    RELEASE_SEED_RUNTIME: '/fixture/elastos', VERIFY_LOG: join(root, 'verify.jsonl'),
    ARGUMENT_LOG: join(root, 'arguments.json') };
  const ciScript = join(repo, 'scripts/checked-ci-fixture.sh');
  writeFileSync(ciScript, readFileSync(join(repo, 'scripts/release-publish.sh'), 'utf8')
    .split('\npublication_record() {')[0] + '\nchecked_ci "$1"\n');
  return { root, env, script: join(repo, 'scripts/release-publish.sh'),
    prepare(version = '1.2.3-alpha.1', selected = [], extraEnv = {}) {
      return spawnSync('bash', [this.script, 'prepare', '123', version, ...selected],
        { encoding: 'utf8', env: { ...env, ...extraEnv } });
    },
    checkedCi() { return spawnSync('bash', [ciScript, commit], { encoding: 'utf8', env }); },
    seedUpgrade() { return spawnSync('bash', [this.script, 'seed-upgrade', '123'], { encoding: 'utf8', env }); },
    args() { return JSON.parse(readFileSync(env.ARGUMENT_LOG)); } };
}

for (const selected of [[], ['aarch64-darwin', 'x86_64-linux'], ['aarch64-darwin']]) {
  test(`prepare passes each platform once: ${selected.join(',') || 'all'}`, t => {
    const f = fixture(t);
    const result = f.prepare('1.2.3-alpha.2', selected);
    assert.equal(result.status, 0, result.stderr);
    const expected = selected.length ? selected : platforms;
    const args = f.args();
    const inputs = args.flatMap((arg, i) => arg === '--platform-input' ? [args[i + 1]] : []);
    assert.deepEqual(inputs, expected.map(p => `${p}=${f.root}/work/1.2.3-alpha.2/inputs/${p}/N1`));
    assert.equal(args.includes('--preview-platform'), expected.length === 1);
    assert.equal(readFileSync(f.env.VERIFY_LOG, 'utf8').trim().split('\n').length, expected.length);
    const record = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.2/run.json')));
    assert.deepEqual(record.ci_run, { id: ciRun.id, url: ciRun.html_url });
    assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
  });
}

test('prepare accepts an exact source develop queue run and retains consumer receipts', t => {
  const f = fixture(t, { ciRuns: [queueRun] });
  const result = f.prepare();
  assert.equal(result.status, 0, result.stderr);
  const record = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.1/run.json')));
  assert.deepEqual(record.ci_run, { id: ciRun.id, url: ciRun.html_url });
  const verified = readFileSync(f.env.VERIFY_LOG, 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(verified.length, platforms.length);
  assert.ok(verified.every(receipt => receipt.source.commit === commit && receipt.source.tree === tree));
});

test('prepare retains distinct approved builder/source identities and receipt proof for the signer', t => {
  const f = fixture(t);
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
  assert.equal(result.status, 0, result.stderr);
  const manifest = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.1/unsigned/signing-input.json')));
  assert.equal(manifest.source.commit, commit);
  assert.notEqual(workflowCommit, commit);
  assert.equal(manifest.provenance.run_id, packageRun.id);
  assert.equal(manifest.provenance.run_attempt, packageRun.run_attempt);
  assert.equal(manifest.provenance.receipts.length, 1);
  const proof = manifest.provenance.receipts[0];
  assert.equal(proof.artifact.name, `release-mac-${commit}-1`);
  assert.ok(readFileSync(join(f.root, 'work/1.2.3-alpha.1/unsigned', proof.receipt)).length);
  assert.ok(readFileSync(join(f.root, 'work/1.2.3-alpha.1/unsigned', proof.bundle)).length);
});

test('prepare preserves accepted native and proof artifacts from earlier attempts', t => {
  const f = fixture(t, { packageRun: { ...packageRun, run_attempt: 3 }, nativeAttempt: 1, proofAttempt: 2 });
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
  assert.equal(result.status, 0, result.stderr);
  const record = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.1/run.json')));
  assert.equal(record.run_attempt, 3);
  assert.equal(record.receipts[0].artifact.attempt, 1);
  assert.equal(record.receipts[0].proof_artifact.attempt, 2);
  assert.deepEqual(JSON.parse(readFileSync(f.env.ARGUMENT_LOG + '.delivery')),
    { gateway: delivery.PUBLISHER_GATEWAY, ipns: delivery.IPNS_NAME });
});

test('prepare refuses receipt producer attempt mismatch before source execution', t => {
  const f = fixture(t, { producer: { attestation_attempt: 2 } });
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /artifact.run.attempt differs/);
  assert.throws(() => f.args(), { code: 'ENOENT' });
});

for (const [label, changes, error] of [
  ['edited workflow branch', { packageRun: { ...packageRun, head_branch: 'feature/edited-workflow' } }, /approved producer/],
  ['unapproved workflow commit', { packageRun: { ...packageRun, head_sha: 'd'.repeat(40) } }, /approved producer/],
  ['fork producer', { packageRun: { ...packageRun, head_repository: { id: 999, full_name: 'attacker/fork' } } }, /approved producer/],
  ['pull request target event', { packageRun: { ...packageRun, event: 'pull_request_target' } }, /approved producer/],
  ['main dispatch', { packageRun: { ...packageRun, head_branch: 'main' } }, /approved producer/],
  ['main producer approval', { approval: { workflow: { commit: workflowCommit, tree: workflowTree, ref: 'refs/heads/main' } } },
    /approved develop producer ref required/],
  ['workflow differs from develop', { workflowBlob: '6'.repeat(40) }, /package workflow differs from develop/],
  ['wrong workflow tree', { workflowTree: 'd'.repeat(40) }, /producer\/source tree/],
  ['missing attestation', { failAttestation: true }, /attestation verification failed/],
  ['self-hosted attestation', { certificate: { runnerEnvironment: 'self-hosted' } }, /certificate/],
  ['wrong attested run', { certificate: { runInvocationURI: 'https://github.com/Elacity/elastos-runtime/actions/runs/999/attempts/1' } }, /certificate/],
  ['wrong attested attempt', { certificate: { runInvocationURI: 'https://github.com/Elacity/elastos-runtime/actions/runs/123/attempts/2' } }, /certificate/],
  ['changed bootstrap node', { bootstrap: { node_id: '5'.repeat(64) } }, /delivery approval/],
  ['changed bootstrap ticket', { bootstrap: { ticket: 'unapproved' } }, /delivery approval/],
]) {
  test(`prepare refuses ${label} before unsigned preparation`, t => {
    const f = fixture(t, changes);
    const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.throws(() => f.args(), { code: 'ENOENT' });
    if (label.startsWith('changed bootstrap')) {
      // The identity check runs before the hours-long input downloads.
      assert.throws(() => readFileSync(join(f.root, 'work/1.2.3-alpha.1/inputs/aarch64-darwin/N1/platform-input.json')),
        { code: 'ENOENT' });
    }
  });
}

for (const [label, changes, error] of [
  ['update journey pin names the public release head', {}, null],
  ['update journey pin names an older release head', { updatePin: { ...updatePin, head_cid: 'QmUjzbkAaxa8StyyqL5qswtvFu2k6b6GqU552KkGLG8omY' } },
    /update journey did not start from the release users run now \(pinned head QmUjzbk\S+, public head QmZ9v8L\S+\)/],
  ['update journey pin is missing', { updatePin: null }, /has no update journey pin/],
  ['oldest System-update pin is missing', { oldestPin: null }, /has no oldest System-update pin/],
  ['oldest System-update pin is another file', { oldestPin: updatePin }, /has no oldest System-update pin/],
]) {
  test(`prepare ${error ? 'refuses before any download' : 'continues'} when the source's ${label}`, t => {
    const f = fixture(t, changes);
    const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
    const calls = readFileSync(join(f.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    const downloads = calls.filter(c => c.command === 'gh' && c.args.some(arg => arg.endsWith('/zip')));
    if (!error) {
      assert.equal(result.status, 0, result.stderr);
      assert.equal(downloads.length, 2);
      return;
    }
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.deepEqual(calls.filter(c => c.command === 'gh'), []);
    assert.equal(existsSync(join(f.env.RELEASE_WORK, '1.2.3-alpha.1')), false);
  });
}

// N+1 of a pair comes from N's source, whose pin still names the head before N.
const headFor = prev => JSON.stringify({ payload: { version: '1.2.3-alpha.1', prev_head_cid: prev }, signer_did: did }) + '\n';
for (const [label, change, accepted] of [
  ['the head this Mac signed for N', () => {}, true],
  ['not the head this Mac signed for N', (n, served) => writeFileSync(served, headFor(publicHead).replace('}', ',"channel":"canary"}')), false],
  ['the head signed for N, which does not follow the pin', (n, served) => {
    for (const path of [join(n, 'signed/release-head.json'), served]) writeFileSync(path, headFor('QmT78zSuBmuS4z925WZfrqQ1qHaJ56DQaTfyMUF7F8ff5o'));
  }, false],
  ['the head signed for N from another package run', n => {
    writeFileSync(join(n, 'run.json'), JSON.stringify({ ...JSON.parse(readFileSync(join(n, 'run.json'))), run: '999' }));
  }, false],
]) {
  test(`prepare N+1 ${accepted ? 'continues' : 'refuses before any download'} once the public head is ${label}`, t => {
    const f = fixture(t);
    assert.equal(f.prepare('1.2.3-alpha.1', ['aarch64-darwin']).status, 0);
    const n = join(f.env.RELEASE_WORK, '1.2.3-alpha.1');
    const served = join(f.root, 'public-head.json');
    mkdirSync(join(n, 'signed'));
    writeFileSync(join(n, 'signed/release-head.json'), headFor(publicHead));
    writeFileSync(served, headFor(publicHead));
    const configPath = join(f.root, 'config.json');
    writeFileSync(configPath, JSON.stringify({ ...JSON.parse(readFileSync(configPath)), headFile: served }));
    change(n, served);
    const calls = () => readFileSync(join(f.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    const before = calls().length;
    const result = f.prepare('1.2.3-alpha.2', ['aarch64-darwin']);
    const downloads = calls().slice(before).filter(c => c.command === 'gh' && c.args.some(arg => arg.endsWith('/zip')));
    if (accepted) {
      assert.equal(result.status, 0, result.stderr);
      assert.equal(downloads.length, 2);
      return;
    }
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /update journey did not start from the release users run now \(pinned head QmZ9v8L/);
    assert.deepEqual(downloads, []);
    assert.equal(existsSync(join(f.env.RELEASE_WORK, '1.2.3-alpha.2')), false);
  });
}

test('prepare requires an independently owned approval outside the candidate work', t => {
  const f = fixture(t);
  const missing = f.prepare('1.2.3-alpha.1', ['aarch64-darwin'], { RELEASE_APPROVAL: '' });
  assert.notEqual(missing.status, 0);
  assert.match(missing.stderr, /set RELEASE_APPROVAL/);
  mkdirSync(f.env.RELEASE_WORK);
  const candidate = join(f.env.RELEASE_WORK, 'approval.json');
  cpSync(f.env.RELEASE_APPROVAL, candidate);
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin'], { RELEASE_APPROVAL: candidate });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /outside candidate inputs/);
});

test('seed-upgrade requires exact-source CI before downloading or printing install commands', t => {
  const f = fixture(t, { ciOnly: true, seedFixture: true, ciRuns: [] });
  const result = f.seedUpgrade();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /successful develop CI run/);
  assert.doesNotMatch(result.stdout, /systemctl|install -m 755/);
  const calls = readFileSync(join(f.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(calls.some(call => call.args.some(arg => arg.endsWith('/zip'))), false);
});

test('seed-upgrade verifies an attested receipt and prints pinned payload hashes/source tree', t => {
  const f = fixture(t, { ciOnly: true, seedFixture: true });
  const result = f.seedUpgrade();
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Develop CI run: 456/);
  assert.match(result.stdout, new RegExp(`tree ${tree}`));
  assert.match(result.stdout, /seed\/elastos' \| sha256sum -c --quiet/);
  assert.match(result.stdout, /seed\/ipfs-provider' \| sha256sum -c --quiet/);
  assert.match(result.stdout, /seed\/source.tar.gz' \| sha256sum -c --quiet/);
  run('bash', ['-n'], { input: result.stdout });
});

test('seed-upgrade refuses an unapproved attestation before printing service mutation', t => {
  const f = fixture(t, { ciOnly: true, seedFixture: true, certificate: { buildSignerDigest: 'd'.repeat(40) } });
  const result = f.seedUpgrade();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /certificate/);
  assert.doesNotMatch(result.stdout, /systemctl|install -m 755/);
});

for (const [label, ciRuns] of [
  ['queue after develop push', [{ ...ciRun, id: 400 }, queueRun]],
  ['develop push after queue', [{ ...queueRun, id: 400 }, ciRun]],
  ['develop push with newer unrelated event', [ciRun, { ...queueRun, id: 999, event: 'pull_request_target' }]],
  ['queue with newer wrong-branch push', [queueRun, { ...ciRun, id: 999, head_branch: 'main' }]],
]) {
  test(`exact source CI guard accepts ${label}`, t => {
    const f = fixture(t, { ciRuns, ciOnly: true });
    const result = f.checkedCi();
    assert.equal(result.status, 0, result.stderr);
    assert.deepEqual(JSON.parse(result.stdout), { id: ciRun.id, url: ciRun.html_url });
  });
}

for (const [label, changes, error] of [
  ['failed CI', { ciRuns: [{ ...ciRun, conclusion: 'failure' }] }, /successful develop CI run/],
  ['no CI run', { ciRuns: [] }, /successful develop CI run/],
  ['skipped required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'skipped' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: skipped/],
  ['failed required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'failure' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: failure/],
  ['missing required job', { ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'success' }] }, /source-home-macos: missing/],
  ['wrong source CI', { ciRuns: [{ ...ciRun, head_sha: 'c'.repeat(40) }] }, /successful develop CI run/],
  ['PR CI', { ciRuns: [{ ...ciRun, event: 'pull_request' }] }, /successful develop CI run/],
  ['wrong branch CI', { ciRuns: [{ ...ciRun, head_branch: 'main' }] }, /successful develop CI run/],
  ['incomplete CI', { ciRuns: [{ ...ciRun, status: 'in_progress' }] }, /successful develop CI run/],
  ['empty required jobs', { requiredJobs: [] }, /cannot read develop required jobs/],
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
    assert.equal(calls.some(c => (c.command === 'git' && c.args.includes('worktree')) || c.command === 'scp'), false);
  });
}

for (const [label, changes, error] of [
  ['wrong SHA', { ciRuns: [{ ...queueRun, head_sha: 'c'.repeat(40) }] }, /successful develop CI run/],
  ['wrong queue branch', { ciRuns: [{ ...queueRun, head_branch: 'gh-readonly-queue/main/pr-1' }] }, /successful develop CI run/],
  ['queue branch prefix collision', { ciRuns: [{ ...queueRun, head_branch: 'gh-readonly-queue/develop-other/pr-1' }] }, /successful develop CI run/],
  ['plain develop branch', { ciRuns: [{ ...queueRun, head_branch: 'develop' }] }, /successful develop CI run/],
  ['wrong workflow', { ciRuns: [{ ...queueRun, path: '.github/workflows/other.yml' }] }, /successful develop CI run/],
  ['fork PR event', { ciRuns: [{ ...queueRun, event: 'pull_request' }] }, /successful develop CI run/],
  ['fork target event', { ciRuns: [{ ...queueRun, event: 'pull_request_target' }] }, /successful develop CI run/],
  ['dispatch event', { ciRuns: [{ ...queueRun, event: 'workflow_dispatch' }] }, /successful develop CI run/],
  ['failed run', { ciRuns: [{ ...queueRun, conclusion: 'failure' }] }, /successful develop CI run/],
  ['incomplete run', { ciRuns: [{ ...queueRun, status: 'in_progress' }] }, /successful develop CI run/],
  ['skipped job', { ciRuns: [queueRun], ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'skipped' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: skipped/],
  ['failed job', { ciRuns: [queueRun], ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'failure' },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: failure/],
  ['missing job', { ciRuns: [queueRun], ciJobs: [{ name: 'test-elastos', status: 'completed', conclusion: 'success' }] }, /source-home-macos: missing/],
  ['incomplete job', { ciRuns: [queueRun], ciJobs: [{ name: 'test-elastos', status: 'in_progress', conclusion: null },
    { name: 'source-home-macos', status: 'completed', conclusion: 'success' }] }, /test-elastos: in_progress/],
  ['failed duplicate job', { ciRuns: [queueRun], ciJobs: [...requiredJobs.map(name => ({ name, status: 'completed', conclusion: 'success' })),
    { name: 'test-elastos', status: 'completed', conclusion: 'failure' }] }, /test-elastos: failure/],
]) {
  test(`exact source CI guard refuses queue CI with ${label}`, t => {
    const f = fixture(t, { ...changes, ciOnly: true });
    const result = f.checkedCi();
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.throws(() => f.args(), { code: 'ENOENT' });
    const calls = readFileSync(join(f.root, 'calls.jsonl'), 'utf8').trim().split('\n').map(JSON.parse);
    assert.equal(calls.some(c => c.command === 'git' || c.command === 'scp'), false);
  });
}

test('prepare refuses an exact queue source outside develop before unsigned preparation', t => {
  const f = fixture(t, { ciRuns: [queueRun], compareStatus: 'behind' });
  const result = f.prepare();
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /source .* is not on develop/);
  assert.throws(() => f.args(), { code: 'ENOENT' });
});

function policy(f, version) {
  const signer = join(f.root, 'signer.py');
  const key = join(f.root, 'fixture-key');
  writeFileSync(signer, '# fixture signer');
  writeFileSync(key, 'fixture key');
  const openssl = run('which', ['openssl']).stdout.trim();
  return spawnSync('bash', [f.script, 'policy', version, signer, key, openssl], { encoding: 'utf8', env: f.env });
}

test('policy prints the prepared source, package run and develop CI run', t => {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const result = policy(f, '1.2.3-alpha.1');
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, new RegExp(`Source commit: ${commit}`));
  assert.match(result.stdout, /Package run: 123 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/123/);
  assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
  const pinned = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.1/policy.json')));
  assert.deepEqual([pinned.channel, pinned.develop_oid, pinned.tag], ['canary', commit, undefined]);
});

// A final version is one stable head: canary Homes accept it, so nothing else is signed.
const stable = { versions: ['1.2.3', '1.2.4-alpha.1'] };

test('a final version prepares on stable from its tag on main and pins that tag for the signer', t => {
  const f = fixture(t, stable);
  const result = f.prepare('1.2.3');
  assert.equal(result.status, 0, result.stderr);
  const args = f.args();
  assert.equal(args[args.indexOf('--channel') + 1], 'stable');
  assert.ok(!args.includes('--preview-platform'));
  assert.match(result.stdout, new RegExp(`Stable tag: v1\\.2\\.3 ${'e'.repeat(40)} on main`));
  const record = JSON.parse(readFileSync(join(f.root, 'work/1.2.3/run.json')));
  assert.deepEqual([record.channel, record.tag], ['stable', { name: 'v1.2.3', oid: 'e'.repeat(40) }]);
  const signed = policy(f, '1.2.3');
  assert.equal(signed.status, 0, signed.stderr);
  const pinned = JSON.parse(readFileSync(join(f.root, 'work/1.2.3/policy.json')));
  assert.deepEqual([pinned.channel, pinned.version, pinned.tag, pinned.tag_oid, pinned.develop_oid],
    ['stable', '1.2.3', 'v1.2.3', 'e'.repeat(40), undefined]);
  const signedDir = join(f.root, 'signed');
  mkdirSync(signedDir);
  writeFileSync(join(signedDir, 'release-head.json'), JSON.stringify({ signer_did: did }));
  const seed = run('bash', [f.script, 'seed', '1.2.3', signedDir], { env: f.env });
  assert.match(seed.stdout, /publish-release --version 1\.2\.3 --channel stable --signed-publication signed/);
});

for (const [label, changes, selected] of [
  ['a missing tag', { noTag: true }, []],
  ['a tag naming another commit', { tagTarget: 'c'.repeat(40) }, []],
  ['a tag object for another name', { tagName: 'v9.9.9' }, []],
  ['a source outside main', { mainCompareStatus: 'diverged' }, []],
  ['main with other changes', { mainTree: 'c'.repeat(40) }, []],
  ['another elastos/Cargo.toml version', { cargoVersion: '0.7.1' }, []],
  ['a single platform', {}, ['aarch64-darwin']],
]) {
  test(`stable prepare refuses ${label} before unsigned preparation`, t => {
    const f = fixture(t, { ...stable, ...changes });
    const result = f.prepare('1.2.3', selected);
    assert.notEqual(result.status, 0, result.stdout);
    assert.throws(() => f.args(), { code: 'ENOENT' });
  });
}

test('the channel follows the SemVer prerelease, not hyphens in build metadata', t => {
  const f = fixture(t, { ciOnly: true });
  const script = join(f.root, 'release-channel.sh');
  writeFileSync(script, readFileSync(f.script, 'utf8').split('\npublication_record() {')[0] + '\nrelease_channel "$1"\n');
  for (const [version, channel] of [
    ['1.2.3', 'stable'], ['1.2.3+build-1', 'stable'], ['1.2.3+build.1-x', 'stable'],
    ['1.2.3-alpha.1', 'canary'], ['1.2.3-rc.1', 'canary'], ['1.2.3-alpha.1+build-1', 'canary'],
  ]) assert.equal(run('bash', [script, version], { env: f.env }).stdout.trim(), channel, version);
});

test('a final version with hyphenated build metadata keeps every stable gate', t => {
  const f = fixture(t, { versions: ['1.2.3+build-1', '1.2.4-alpha.1'], noTag: true });
  const result = f.prepare('1.2.3+build-1');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /tag v1\.2\.3\+build-1 is missing/);
  assert.throws(() => f.args(), { code: 'ENOENT' });
});

test('stable policy refuses a tag moved after preparation', t => {
  const f = fixture(t, stable);
  assert.equal(f.prepare('1.2.3').status, 0);
  const configPath = join(f.root, 'config.json');
  writeFileSync(configPath, JSON.stringify({ ...JSON.parse(readFileSync(configPath)), tagOid: 'f'.repeat(40) }));
  const result = policy(f, '1.2.3');
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /tag v1\.2\.3 moved after preparation/);
  assert.throws(() => readFileSync(join(f.root, 'work/1.2.3/policy.json')), { code: 'ENOENT' });
});

for (const selected of [['x86_64-linux'], ['aarch64-darwin', 'aarch64-darwin'], ['unknown']]) {
  test(`prepare refuses invalid platform selection: ${selected.join(',')}`, t => {
    const f = fixture(t);
    assert.notEqual(f.prepare('1.2.3-alpha.1', selected).status, 0);
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
    assert.notEqual(f.prepare('1.2.3-alpha.1', [], extraEnv).status, 0);
    assert.throws(() => f.args(), { code: 'ENOENT' });
  });
}

test('seed refuses a rerun artifact that differs from the prepared signed candidate', t => {
  const f = fixture(t);
  const prepared = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
  assert.equal(prepared.status, 0, prepared.stderr);
  const configPath = join(f.root, 'config.json');
  const config = JSON.parse(readFileSync(configPath));
  config.packageRun = { ...config.packageRun, run_attempt: 2 };
  config.artifacts['release-mac'] = { ...config.artifacts['release-mac'], id: '501', attempt: 2 };
  writeFileSync(configPath, JSON.stringify(config));
  const signed = join(f.root, 'signed');
  mkdirSync(signed);
  writeFileSync(join(signed, 'release-head.json'), JSON.stringify({ signer_did: did }));
  const result = spawnSync('bash', [f.script, 'seed', '1.2.3-alpha.1', signed], { encoding: 'utf8', env: f.env });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /changed after preparation/);
  assert.doesNotMatch(result.stdout, /systemctl|scp |get seed|On the seed/);
});

// The signed set a real run leaves: native artifacts the run carries and files only the Mac has.
function signedSet(f, head) {
  const signed = join(f.root, 'signed');
  mkdirSync(signed);
  writeFileSync(join(signed, 'release-head.json'), JSON.stringify(head));
  writeFileSync(join(signed, 'install.sh'), 'signed installer');
  writeFileSync(join(signed, 'release.json'), 'signed release');
  for (const p of platforms) {
    writeFileSync(join(signed, `components-${p}.json`), `signed components ${p}`);
    cpSync(join(f.root, p, 'inputs/N/artifacts', `elastos-${p}`), join(signed, `elastos-${p}`));
  }
  cpSync(join(f.root, 'aarch64-darwin/inputs/N/artifacts/shared.tar.gz'), join(signed, 'shared.tar.gz'));
  // A signed file no native input carries.
  writeFileSync(join(signed, 'mac-only.bin'), 'mac only');
  return signed;
}

test('seed reconstructs all platform files and copies every other signed file from the Mac', t => {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const signed = signedSet(f, { signer_did: did });
  writeState(f.env.RELEASE_SEED_DATA, {});
  const result = run('bash', [f.script, 'seed', '1.2.3-alpha.1', signed], { env: f.env });
  assert.match(result.stdout, new RegExp(`Source commit: ${commit}`));
  assert.match(result.stdout, /Package run: 123 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/123/);
  assert.match(result.stdout, /Develop CI run: 456 https:\/\/github.com\/Elacity\/elastos-runtime\/actions\/runs\/456/);
  run('bash', ['-n'], { input: result.stdout });
  const stage = join(f.root, 'stage/1.2.3-alpha.1');
  mkdirSync(join(stage, 'signed'), { recursive: true });
  // Copy exactly what the printed Mac scp line names.
  const copied = result.stdout.match(/^scp (.+) \S+:\S+\/signed\/$/m)[1].split(' ');
  for (const file of copied) cpSync(file, join(stage, 'signed', file.split('/').at(-1)));
  assert.ok(copied.every(file => !/elastos-|shared/.test(file.split('/').at(-1))), 'native artifacts come from the run');
  cpSync(join(f.root, 'work/1.2.3-alpha.1/signed.SHA256SUMS'), join(stage, 'signed.SHA256SUMS'));
  // Run only artifact reconstruction. Service and publication commands stay printed.
  const reconstruction = result.stdout.split('# On the seed as the service user,')[1].split('export ELASTOS_DATA_DIR=')[0];
  run('bash', ['-c', '#' + reconstruction], { env: { ...f.env, GH_TOKEN: 'fixture' } });
  for (const p of platforms) assert.equal(readFileSync(join(stage, 'signed', `elastos-${p}`), 'utf8'), `${p}-1.2.3-alpha.1`);
  assert.equal(readFileSync(join(stage, 'signed/mac-only.bin'), 'utf8'), 'mac only');
});

for (const [label, head, error] of [
  ['prev_head_cid', { signer_did: did, payload: { prev_head_cid: `x'$(touch PWNED)'` } }, /prev_head_cid is not a canonical CID/],
  ['signer_did', { signer_did: `${did}$(touch PWNED)`, payload: {} }, /signer_did is not a did:key/],
]) {
  test(`seed refuses a signed head whose ${label} is not plain data and prints no command`, t => {
    const f = fixture(t);
    assert.equal(f.prepare().status, 0);
    const result = spawnSync('bash', [f.script, 'seed', '1.2.3-alpha.1', signedSet(f, head)], { encoding: 'utf8', env: f.env });
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.equal(result.stdout, '');
  });
}

test('seed prints each value as one shell word, so the whole printed sequence runs nothing a value carries', t => {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const signed = signedSet(f, { signer_did: did });
  const evil = `q'"$(touch PWNED)\`touch PWNED\`;x`;
  const tools = join(f.root, 'seed-tools');
  const log = join(f.root, 'seed.log');
  mkdirSync(tools);
  // ssh and scp without a network: the remote command runs in a local shell.
  writeFileSync(join(tools, 'ssh'), '#!/bin/bash\nshift\nexec bash -c "$*"\n', { mode: 0o755 });
  writeFileSync(join(tools, 'scp'), '#!/bin/bash\nset -e\ndest=${@: -1}\nfor file in "${@:1:$#-1}"; do cp "$file" "${dest#*:}"; done\n', { mode: 0o755 });
  writeFileSync(join(tools, 'sudo'), '#!/bin/sh\n[ "$1" = -v ] || exec "$@"\n', { mode: 0o755 });
  writeFileSync(join(tools, 'systemctl'), `#!/bin/sh\nprintf 'systemctl %s\\n' "$*" >> '${log}'\n`, { mode: 0o755 });
  const env = { ...f.env, GH_TOKEN: 'fixture', PATH: `${tools}:${f.env.PATH}`, RELEASE_SEED: `seed${evil}`,
    RELEASE_SEED_DATA: join(f.root, `data ${evil}`, 'elastos'), RELEASE_SEED_STAGE: join(f.root, `stage ${evil}`),
    RELEASE_SEED_RUNTIME: join(f.root, `runtime ${evil}`, 'elastos'), RELEASE_SEED_UNIT: `elastos${evil}.service` };
  mkdirSync(join(env.RELEASE_SEED_DATA, 'bin'), { recursive: true });
  writeFileSync(join(env.RELEASE_SEED_DATA, 'bin/ipfs-provider'), '', { mode: 0o755 });
  mkdirSync(dirname(env.RELEASE_SEED_RUNTIME));
  writeFileSync(env.RELEASE_SEED_RUNTIME, `#!/bin/sh\nprintf 'runtime %s\\n' "$*" >> '${log}'\n`, { mode: 0o755 });
  const printed = run('bash', [f.script, 'seed', '1.2.3-alpha.1', signed], { env }).stdout;
  const result = spawnSync('bash', ['-c', printed], { encoding: 'utf8', cwd: f.root, env });
  assert.equal(result.status, 0, result.stderr);
  assert.deepEqual(readdirSync(f.root, { recursive: true }).filter(path => path.split('/').at(-1) === 'PWNED'), []);
  const publish = `runtime publish-release --version 1.2.3-alpha.1 --channel canary --signed-publication signed --publisher-did ${did} `
    + `--ipfs-provider-bin ${env.RELEASE_SEED_DATA}/bin/ipfs-provider`;
  assert.deepEqual(readFileSync(log, 'utf8').trim().split('\n'), [`systemctl stop ${env.RELEASE_SEED_UNIT}`,
    `${publish} --preflight-only`, publish, `systemctl start ${env.RELEASE_SEED_UNIT}`]);
  for (const p of platforms) assert.equal(readFileSync(join(env.RELEASE_SEED_STAGE, '1.2.3-alpha.1/signed', `elastos-${p}`), 'utf8'), `${p}-1.2.3-alpha.1`);
});

function writeState(data, state) {
  const dir = join(data, 'ElastOS/SystemServices/Publisher');
  mkdirSync(dir, { recursive: true });
  writeFileSync(join(dir, 'publish-state.json'), JSON.stringify(state));
}

function seedScript(t, head, extraEnv = {}) {
  const f = fixture(t);
  assert.equal(f.prepare().status, 0);
  const signed = join(f.root, 'signed');
  mkdirSync(signed);
  writeFileSync(join(signed, 'release-head.json'), JSON.stringify(head));
  const env = { ...f.env, ...extraEnv };
  const stdout = run('bash', [f.script, 'seed', '1.2.3-alpha.1', signed], { env }).stdout;
  return { f, env, stdout };
}

test('seed runs the Runtime with the data dir it resolves from XDG_DATA_HOME', t => {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'release-seed-xdg-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const runtime = join(root, 'elastos');
  const calls = join(root, 'runtime.log');
  const { env, stdout } = seedScript(t, { signer_did: did }, { RELEASE_SEED_RUNTIME: runtime });
  const data = env.RELEASE_SEED_DATA;
  // Like dirs::data_dir()/elastos: ELASTOS_DATA_DIR does not decide where publish state is read.
  writeFileSync(runtime, `#!/bin/sh\n[ "\${XDG_DATA_HOME:-$HOME/.local/share}/elastos" = '${data}' ] || exit 7\necho "$*" >> '${calls}'\n`, { mode: 0o755 });
  mkdirSync(join(data, 'bin'), { recursive: true });
  writeFileSync(join(data, 'bin/ipfs-provider'), '', { mode: 0o755 });
  const bin = join(root, 'bin');
  mkdirSync(bin);
  writeFileSync(join(bin, 'sudo'), '#!/bin/sh\n[ "$1" = -v ] || exec "$@"\n', { mode: 0o755 });
  writeFileSync(join(bin, 'systemctl'), '#!/bin/sh\n', { mode: 0o755 });
  const publication = 'set -euo pipefail\nexport ELASTOS_DATA_DIR=' + stdout.split('export ELASTOS_DATA_DIR=')[1];
  const runEnv = { ...env, PATH: `${bin}:${env.PATH}`, HOME: join(root, 'home') };
  delete runEnv.XDG_DATA_HOME;
  const result = spawnSync('bash', ['-c', publication], { encoding: 'utf8', cwd: root, env: runEnv });
  assert.equal(result.status, 0, result.stderr);
  const lines = readFileSync(calls, 'utf8').trim().split('\n');
  assert.equal(lines.length, 2);
  assert.match(lines[0], /^publish-release .* --preflight-only$/);
});

const priorHead = 'QmUjzbkAaxa8StyyqL5qswtvFu2k6b6GqU552KkGLG8omY';
for (const [label, payload, cases] of [
  ['a signed head with a predecessor', { prev_head_cid: priorHead }, [
    [{ last_head_cid: publicHead, last_version: '1.2.2' }, false],
    [{ last_head_cid: priorHead, last_version: '1.2.2' }, true],
    // A rerun after commit; the Runtime checks the exact head CID itself.
    [{ last_head_cid: publicHead, last_version: '1.2.3-alpha.1' }, true],
    [null, false],
  ]],
  // The Runtime reads absent publish state as empty.
  ['the first signed head', {}, [
    [null, true],
    [{}, true],
    [{ last_head_cid: priorHead, last_version: '1.2.2' }, false],
  ]],
]) {
  test(`seed downloads for ${label} only when saved state precedes it or holds this version`, t => {
    const { f, env, stdout } = seedScript(t, { signer_did: did, payload });
    const bin = join(f.root, 'download-bin');
    const downloads = join(f.root, 'downloads.log');
    mkdirSync(bin);
    writeFileSync(join(bin, 'curl'), `#!/bin/sh\necho "$*" >> '${downloads}'\nexit 1\n`, { mode: 0o755 });
    const seedSide = '#' + stdout.split('# On the seed as the service user,')[1];
    for (const [state, downloaded] of cases) {
      rmSync(downloads, { force: true });
      if (state) writeState(env.RELEASE_SEED_DATA, state);
      else rmSync(join(env.RELEASE_SEED_DATA, 'ElastOS/SystemServices/Publisher/publish-state.json'), { force: true });
      const result = spawnSync('bash', ['-c', seedSide], { encoding: 'utf8', env: { ...env, GH_TOKEN: 'fixture', PATH: `${bin}:${env.PATH}` } });
      assert.notEqual(result.status, 0);
      assert.equal(/prev_head_cid/.test(result.stderr), !downloaded, result.stderr);
      assert.equal(existsSync(downloads), downloaded, JSON.stringify(state));
    }
  });
}

for (const data of ['relative/elastos', '/srv/elastos-data', '/srv/elastos/']) {
  test(`seed and seed-upgrade refuse RELEASE_SEED_DATA ${data}`, t => {
    const f = fixture(t, { ciOnly: true, seedFixture: true });
    for (const args of [['seed', '1.2.3-alpha.1', f.root], ['seed-upgrade', '123']]) {
      const result = spawnSync('bash', [f.script, ...args], { encoding: 'utf8', env: { ...f.env, RELEASE_SEED_DATA: data } });
      assert.notEqual(result.status, 0);
      assert.match(result.stderr, /RELEASE_SEED_DATA must be an absolute path ending in \/elastos/);
    }
  });
}

// The custodian community-profile output: the startup file and its release pin fragment.
const rawCid = bytes => {
  const bits = [...Buffer.concat([Buffer.from([1, 0x55, 0x12, 0x20]), createHash('sha256').update(bytes).digest()])]
    .map(byte => byte.toString(2).padStart(8, '0')).join('');
  return 'b' + bits.match(/.{1,5}/g).map(group => 'abcdefghijklmnopqrstuvwxyz234567'[parseInt(group.padEnd(5, '0'), 2)]).join('');
};
const otherDid = 'did:key:z6MkiTBz1ymuepAQ4HEHYSF1H8quG5GLVVQR3djdX3mDooWp';
function communityOutput(networkId = 'fixture-community', signers = [did]) {
  const file = JSON.stringify({ schema: 'elastos.collaboration-network.startup-config/v1', expected_network_id: networkId,
    trusted_profile_signer_dids: signers, profile_chain_base64: ['Zml4dHVyZQ=='], default_conversation_grant_base64: 'Z3JhbnQ=' });
  return { file, pin: { head_cid: rawCid(Buffer.from(file)), expected_network_id: networkId, trusted_profile_signer_dids: signers } };
}
function custodianDir(f, { file, pin }, extra = false) {
  const dir = join(f.root, 'community-output');
  mkdirSync(dir, { mode: 0o700 });
  writeFileSync(join(dir, 'collaboration-network-release-v1.json'), file, { mode: 0o444 });
  writeFileSync(join(dir, 'collaboration-network-release-pin-v1.json'), JSON.stringify({ collaboration_network: pin }), { mode: 0o444 });
  if (extra) writeFileSync(join(dir, 'notes.txt'), 'operator notes', { mode: 0o444 });
  return dir;
}
const runtimeCalls = f => existsSync(join(f.root, 'runtime.jsonl'))
  ? readFileSync(join(f.root, 'runtime.jsonl'), 'utf8').trim().split('\n').map(JSON.parse) : [];

test('prepare names no Community network when neither the source nor the operator names one', t => {
  const f = fixture(t);
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin']);
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, /Community: this release names no network\./);
  assert.equal(existsSync(join(f.root, 'work/1.2.3-alpha.1/unsigned/collaboration-network-release-v1.json')), false);
  assert.deepEqual(runtimeCalls(f), []);
});

test('prepare admits the custodian Community output the approved source pins and verifies it with the release Runtime', t => {
  const output = communityOutput();
  const f = fixture(t, { network: output });
  const result = f.prepare('1.2.3-alpha.1', [], { RELEASE_COMMUNITY_DIR: custodianDir(f, output) });
  assert.equal(result.status, 0, result.stderr);
  assert.match(result.stdout, new RegExp(`names network fixture-community \\(head ${output.pin.head_cid}\\), profile signer ${did}, verified by the release Runtime`));
  const unsigned = join(f.root, 'work/1.2.3-alpha.1/unsigned');
  assert.equal(readFileSync(join(unsigned, 'collaboration-network-release-v1.json'), 'utf8'), output.file);
  for (const platform of platforms) {
    assert.deepEqual(JSON.parse(readFileSync(join(unsigned, `components-${platform}.json`))).collaboration_network, output.pin);
  }
  const calls = runtimeCalls(f);
  assert.equal(calls.length, 1);
  assert.deepEqual(calls[0].args.slice(0, 3), ['collaboration-config', 'verify', '--input']);
  assert.equal(calls[0].home, join(f.root, 'work/1.2.3-alpha.1/community/home'));
  assert.equal(readFileSync(calls[0].args[3], 'utf8'), output.file);
});

// A refused admission leaves the input without package provenance, which the signer refuses, and policy stops.
function assertUnsignable(f) {
  const manifest = JSON.parse(readFileSync(join(f.root, 'work/1.2.3-alpha.1/unsigned/signing-input.json')));
  assert.equal('provenance' in manifest, false);
  const policy = spawnSync('bash', [f.script, 'policy', '1.2.3-alpha.1', '/fixture/signer.py', '/fixture/key', '/fixture/openssl'],
    { encoding: 'utf8', env: f.env });
  assert.notEqual(policy.status, 0);
  assert.match(policy.stderr, /prepare 1\.2\.3-alpha\.1 did not finish/);
}

for (const [label, source, custodian, changes, error, verified] of [
  ['the source pins a network but no custodian output is named', communityOutput(), null, {}, /set RELEASE_COMMUNITY_DIR/, false],
  ['custodian output is named but the source pins no network', null, communityOutput(), {}, /pins no Community network/, false],
  ['the custodian file changed after signing', communityOutput(),
    { ...communityOutput(), file: communityOutput().file.replace('fixture-community', 'fixture-communitx') }, {},
    /Community network head differs from its file/, false],
  ['the source pins another signed network', communityOutput('other-community'), communityOutput(), {},
    /pins another Community network than the custodian output/, false],
  ['the custodian pin names another network ID', communityOutput(),
    { ...communityOutput(), pin: { ...communityOutput().pin, expected_network_id: 'other-community' } }, {},
    /Community network ID differs from the release pin/, false],
  ['the custodian pin names another network signer', communityOutput(),
    { ...communityOutput(), pin: { ...communityOutput().pin, trusted_profile_signer_dids: [otherDid] } }, {},
    /Community signer set differs from the release pin/, false],
  ['the release Runtime refuses the profile signature', communityOutput(), communityOutput(), { runtimeRefuses: true },
    /release Runtime refused the Community network: profile signer is not trusted/, true],
  ['the release Runtime verifies a profile signer outside the pin', communityOutput(), communityOutput(), { runtimeSigner: otherDid },
    /release Runtime verified another Community network than the release pin/, true],
]) {
  test(`prepare refuses Community admission when ${label}`, t => {
    const f = fixture(t, { ...(source ? { network: source } : {}), ...changes });
    const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin'], custodian ? { RELEASE_COMMUNITY_DIR: custodianDir(f, custodian) } : {});
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, error);
    assert.match(result.stderr, /Community network admission refused/);
    assert.equal(runtimeCalls(f).length, verified ? 1 : 0);
    assert.doesNotMatch(result.stdout, /Unsigned input:/);
    assertUnsignable(f);
  });
}

test('prepare refuses a custodian directory that holds other files', t => {
  const output = communityOutput();
  const f = fixture(t, { network: output });
  const result = f.prepare('1.2.3-alpha.1', ['aarch64-darwin'], { RELEASE_COMMUNITY_DIR: custodianDir(f, output, true) });
  assert.notEqual(result.status, 0);
  assert.match(result.stderr, /holding exactly collaboration-network-release-v1.json and collaboration-network-release-pin-v1.json/);
  assert.deepEqual(runtimeCalls(f), []);
  assertUnsignable(f);
});
