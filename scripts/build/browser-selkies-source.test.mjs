import assert from 'node:assert/strict';
import { cpSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync, symlinkSync, existsSync } from 'node:fs';
import { createHash } from 'node:crypto';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { spawnSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const vendor = fileURLToPath(new URL('../../third-party/selkies', import.meta.url));
const helper = fileURLToPath(new URL('./prepare-browser-selkies.py', import.meta.url));
const builderPath = fileURLToPath(new URL('./build-browser-vm-rootfs.sh', import.meta.url));
const builder = readFileSync(builderPath, 'utf8');
const digest = bytes => createHash('sha256').update(bytes).digest('hex');

function run(root, out) {
  const result = spawnSync('python3', [helper, '--vendor', root, ...(out ? ['--out-dir', out] : [])],
    { encoding: 'utf8', timeout: 10000 });
  assert.ifError(result.error);
  return result;
}

function fixture(callback) {
  const scratch = mkdtempSync(join(tmpdir(), 'selkies-source-test-'));
  try {
    const root = join(scratch, 'vendor');
    cpSync(vendor, root, { recursive: true });
    callback(root, join(scratch, 'prepared'));
  } finally { rmSync(scratch, { recursive: true, force: true }); }
}

test('builder installs local Selkies with dependency and index access disabled', () => {
  assert.doesNotMatch(builder, /selkies==|selkies-gstreamer-web_.*tar\.gz|SELKIES_WEB_URL|selkies-web-url/);
  assert.match(builder, /prepare-browser-selkies\.py/);
  assert.match(builder, /--no-index --no-deps --no-build-isolation.*\/opt\/selkies-build/);
});

test('offline provenance check and patches pass; prepared media bytes match the builder contract', () => {
  fixture((root, out) => {
    // Operators can put output under a checkout. Patch only the build copy.
    const outer = join(out, '..');
    assert.equal(spawnSync('git', ['init', '-q', outer]).status, 0);
    const result = run(root, out);
    assert.equal(result.status, 0, result.stderr);
    const media = readFileSync(join(out, 'src/selkies_gstreamer/gstwebrtc_app.py'));
    assert.match(media.toString(), /# Modified by Elacity for ElastOS: enforce ICE policy and adapt split audio\/video pipelines\./);
    assert.equal(digest(media), '18bb6c7285365fb7fabd5d778341b45226a9254f4d31c06cb565ebdc39d00e6c');
    const manifest = JSON.parse(readFileSync(join(root, 'provenance.json')));
    const web = readFileSync(join(out, 'gst-web/index.html'), 'utf8');
    assert.ok(web.includes(`?ts=${manifest.upstream_commit}`));
    assert.ok(!web.includes('?ts=1"'));
    assert.equal(readFileSync(join(out, 'gst-web/css/font/KFOmCnqEu92Fr1Me5Q.ttf')).compare(
      readFileSync(join(root, 'upstream/addons/gst-web/src/css/font/KFOmCnqEu92Fr1Me5Q.ttf'))), 0);
  });
});

test('a changed vendored byte fails before preparation', () => {
  fixture((root, out) => {
    const name = join(root, 'upstream/src/selkies_gstreamer/gstwebrtc_app.py');
    const bytes = readFileSync(name); bytes[100] ^= 1; writeFileSync(name, bytes);
    const result = run(root, out);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /SHA-256 mismatch/);
  });
});

test('recorded font fields match the TTF name tables and preserve the older Roboto license', () => {
  const result = spawnSync('python3', ['-c', `
import json, runpy, sys
from pathlib import Path
helper = runpy.run_path(sys.argv[1])
vendor = Path(sys.argv[2])
manifest = json.loads((vendor / 'provenance.json').read_text())
for name, record in manifest['fonts'].items():
    assert helper['font_name_fields']((vendor / name).read_bytes()) == record['name_fields']
print(json.dumps(manifest['fonts']))
`, helper, vendor], { encoding: 'utf8', timeout: 10000 });
  assert.ifError(result.error);
  assert.equal(result.status, 0, result.stderr);
  const fonts = Object.values(JSON.parse(result.stdout));
  assert.equal(fonts.length, 7);
  const roboto = fonts.filter(font => font.name_fields.family[0].startsWith('Roboto'));
  assert.equal(roboto.length, 6);
  for (const font of roboto) {
    assert.deepEqual(font.name_fields.license_url, ['http://www.apache.org/licenses/LICENSE-2.0']);
    assert.deepEqual(font.name_fields.license_description, []);
    assert.equal(font.license, 'Apache-2.0');
  }
  const icons = fonts.find(font => font.name_fields.family[0] === 'Material Icons');
  assert.deepEqual(icons.name_fields.license_url, []);
  assert.deepEqual(icons.name_fields.license_description, []);
  assert.deepEqual(icons.name_fields.copyright, ['Copyright 2018 Google, Inc. All Rights Reserved.']);
  assert.equal(icons.license_basis, 'operator-supplied upstream license; font has no license name fields');
});

test('incorrect font metadata fails even when every vendored file hash matches', () => {
  fixture((root, out) => {
    const path = join(root, 'provenance.json');
    const manifest = JSON.parse(readFileSync(path));
    const font = Object.values(manifest.fonts)[0];
    font.name_fields.license_url = ['https://openfontlicense.org'];
    writeFileSync(path, JSON.stringify(manifest));
    assert.match(run(root, out).stderr, /font name metadata mismatch/);
  });
});

test('builder rejects changed source before Cargo or guest network work', () => {
  fixture((root, out) => {
    const repo = join(out, '..', 'repo');
    const build = join(repo, 'scripts/build');
    mkdirSync(build, { recursive: true });
    mkdirSync(join(repo, 'third-party'), { recursive: true });
    cpSync(root, join(repo, 'third-party/selkies'), { recursive: true });
    cpSync(helper, join(build, 'prepare-browser-selkies.py'));
    cpSync(builderPath, join(build, 'build-browser-vm-rootfs.sh'));
    const inputsPath = fileURLToPath(new URL('../browser-vm-image-inputs.py', import.meta.url));
    const declared = spawnSync('python3', ['-c', `import importlib.util,json; s=importlib.util.spec_from_file_location('inputs', ${JSON.stringify(inputsPath)}); m=importlib.util.module_from_spec(s); s.loader.exec_module(m); print(json.dumps(m.REQUIRED_FILES))`],
      { encoding: 'utf8', timeout: 5000 });
    assert.equal(declared.status, 0, declared.stderr);
    cpSync(inputsPath, join(repo, 'scripts/browser-vm-image-inputs.py'));
    const sourceRoot = fileURLToPath(new URL('../../', import.meta.url));
    for (const name of JSON.parse(declared.stdout)) {
      mkdirSync(join(repo, name, '..'), { recursive: true });
      cpSync(join(sourceRoot, name), join(repo, name));
    }
    for (const args of [['init', '-q', repo], ['-C', repo, 'add', '.']]) {
      const result = spawnSync('git', args, { encoding: 'utf8', timeout: 5000 });
      assert.equal(result.status, 0, result.stderr);
    }

    writeFileSync(join(repo, 'third-party/selkies/upstream/setup.cfg'), 'changed source');
    const bin = join(repo, 'bin');
    mkdirSync(bin);
    const marker = join(repo, 'external-work-started');
    // Presence checks pass. Executing Cargo, debootstrap, or root work fails.
    for (const name of ['cargo', 'debootstrap', 'sudo', 'mke2fs', 'cpio', 'gzip']) {
      writeFileSync(join(bin, name), `#!/bin/sh\ntouch '${marker}'\nexit 99\n`, { mode: 0o755 });
    }
    writeFileSync(join(bin, 'findmnt'), '#!/bin/sh\nexit 1\n', { mode: 0o755 });
    const result = spawnSync('bash', [join(build, 'build-browser-vm-rootfs.sh'), '--out-dir', out],
      { encoding: 'utf8', timeout: 10000, env: { ...process.env, PATH: `${bin}:${process.env.PATH}` } });
    assert.ifError(result.error);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /SHA-256 mismatch/);
    assert.equal(existsSync(marker), false);
  });
});

test('unlisted files and symlinks fail closed', () => {
  fixture((root, out) => {
    const name = join(root, 'upstream/extra.py');
    writeFileSync(name, 'unlisted bytes');
    assert.match(run(root, out).stderr, /file list mismatch/);
    rmSync(name); symlinkSync('setup.cfg', name);
    assert.match(run(root, out).stderr, /symlink is forbidden/);
  });
});

test('a rehashed patch that stops applying still fails', () => {
  fixture((root, out) => {
    const name = 'patches/elastos-media.patch';
    const path = join(root, name);
    const patch = readFileSync(path, 'utf8').replace('self.rtpgccbwe = None', 'self.missing_patch_anchor = None');
    writeFileSync(path, patch);
    const manifestPath = join(root, 'provenance.json');
    const manifest = JSON.parse(readFileSync(manifestPath));
    manifest.files[name].sha256 = digest(patch);
    writeFileSync(manifestPath, JSON.stringify(manifest));
    const result = run(root, out);
    assert.notEqual(result.status, 0);
    assert.match(result.stderr, /patch does not apply/);
  });
});
