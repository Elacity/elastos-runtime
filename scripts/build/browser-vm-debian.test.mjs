import assert from 'node:assert/strict';
import fs from 'node:fs';
import test from 'node:test';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';

const script = fileURLToPath(new URL('./browser-vm-debian.py', import.meta.url));
const lock = JSON.parse(fs.readFileSync(new URL('./browser-vm-debian-lock.json', import.meta.url), 'utf8'));
const mirror = `https://snapshot.debian.org/archive/debian/${lock.debian_snapshot}/`;
const run = (...args) => spawnSync('python3', [script, ...args], { encoding: 'utf8', timeout: 5000 });

test('guest admits its frozen Debian sources and rejects a moving mirror or suite', () => {
  assert.equal(run('validate', 'bookworm', mirror).status, 0);
  assert.notEqual(run('validate', 'bookworm', 'https://deb.debian.org/debian').status, 0);
  assert.notEqual(run('validate', 'trixie', mirror).status, 0);
  const sources = run('sources');
  assert.equal(sources.status, 0, sources.stderr);
  assert.match(sources.stdout, /check-valid-until=no/);
  assert.doesNotMatch(sources.stdout, /trusted=yes|allow-unauthenticated/);
  assert.ok(sources.stdout.includes(lock.security_snapshot));
  const packages = run('packages');
  assert.equal(packages.status, 0);
  for (const p of lock.packages) assert.ok(packages.stdout.includes(`${p.name}=${p.version}`));
});

test('guest refuses missing, changed or aliased Chromium archives before installation', () => {
  const code = `import importlib.util, pathlib, tempfile, hashlib
spec=importlib.util.spec_from_file_location('debian', ${JSON.stringify(script)})
m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
with tempfile.TemporaryDirectory() as root:
 p=pathlib.Path(root); payload=b'verified package'
 lock={'packages':[{'name':'chromium','filename':'pool/chromium.deb','size':len(payload),'sha256':hashlib.sha256(payload).hexdigest()}]}
 archive=p/'chromium.deb'
 for mode in ['missing','changed','symlink','good']:
  archive.unlink(missing_ok=True)
  if mode=='changed': archive.write_bytes(b'x'*len(payload))
  elif mode=='symlink':
   target=p/'target'; target.write_bytes(payload); archive.symlink_to(target)
  elif mode=='good': archive.write_bytes(payload)
  try: m.verify_cache(p,lock)
  except ValueError: assert mode!='good'
  else: assert mode=='good'
`;
  const result = spawnSync('python3', ['-c', code], { encoding: 'utf8', timeout: 5000 });
  assert.equal(result.status, 0, result.stderr);
});
