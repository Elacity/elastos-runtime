#!/usr/bin/env python3
"""Inert source-builder fixtures; all payload and licence inputs are local files."""

import hashlib
import io
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import textwrap
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
PLATFORM = 'linux-amd64'


class ReleaseWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='release-workflow-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.bin = self.root / 'bin'
        self.bin.mkdir(mode=0o700)
        self.calls = self.root / 'calls'
        self.output = self.root / 'output'
        self.commit, self.tree = 'a' * 40, 'b' * 40
        self.env = {**os.environ, 'PATH': str(self.bin) + os.pathsep + os.environ['PATH'],
                    'SOURCE_COMMIT': self.commit, 'SOURCE_TREE': self.tree,
                    'INSTALL_VERSION': '1.2.3', 'UPDATE_VERSION': '1.2.4',
                    'GITHUB_REPOSITORY': 'fixture/runtime', 'GITHUB_OUTPUT': str(self.output),
                    'GH_CALLS': str(self.calls), 'GH_TREE': self.tree, 'GH_ON_DEVELOP': 'true',
                    'PYTHONDONTWRITEBYTECODE': '1'}
        gh = self.bin / 'gh'
        gh.write_text('''#!/usr/bin/env bash
set -euo pipefail
printf '%s\\n' "$*" >> "$GH_CALLS"
[[ "$1" == api ]]
case "$2" in
  */git/commits/*) [[ "${GH_ERROR:-}" != tree ]] || exit 42; echo "$GH_TREE" ;;
  */compare/develop...*) echo 0 ;;
  */compare/*...develop) [[ "${GH_ERROR:-}" != compare ]] || exit 42; echo "$GH_ON_DEVELOP" ;;
  */commits/*/pulls) echo 1 ;;
  *) exit 99 ;;
esac
''')
        gh.chmod(0o700)

    def workflow_script(self, job, name, workflow='release-package.yml'):
        source = (ROOT / '.github/workflows' / workflow).read_text()
        body = re.split(r'\n  (?=\S)', source.split('\n  ' + job + ':\n', 1)[1], maxsplit=1)[0]
        marker = '      - ' + name + '\n'
        step = body.split(marker, 1)[1].split('\n      - ', 1)[0]
        return textwrap.dedent(step.split('        run: |\n', 1)[1])

    def admit(self, **overrides):
        return subprocess.run(['bash', '-euo', 'pipefail', '-c',
                               self.workflow_script('admit', 'id: admit')], cwd=self.root,
                              env={**self.env, **overrides}, capture_output=True, text=True,
                              timeout=10)

    def test_release_admits_develop_ancestor(self):
        result = self.admit()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.output.read_text(), 'source_tree=' + self.tree + '\n')
        self.assertEqual(len(self.calls.read_text().splitlines()), 2)

    def test_release_rejects_open_pr_head_before_output(self):
        result = self.admit(GH_ON_DEVELOP='false')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())
        self.assertNotIn('/pulls', self.calls.read_text())

    def test_release_rejects_invalid_or_equal_versions_before_api(self):
        for overrides in ({'INSTALL_VERSION': '01.2.3'}, {'UPDATE_VERSION': 'broken'},
                          {'UPDATE_VERSION': '1.2.3'}, {'SOURCE_COMMIT': 'short'}):
            with self.subTest(overrides=overrides):
                result = self.admit(**overrides)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.calls.exists())
                self.assertFalse(self.output.exists())

    def test_release_rejects_api_errors_before_output(self):
        for endpoint in ('tree', 'compare'):
            with self.subTest(endpoint=endpoint):
                result = self.admit(GH_ERROR=endpoint)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(self.output.exists())

    def test_release_rejects_invalid_api_tree(self):
        result = self.admit(GH_TREE='null')
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.output.exists())
        self.assertEqual(len(self.calls.read_text().splitlines()), 1)

    def engine_fixture(self, cached):
        scripts = self.root / 'scripts'
        (scripts / 'build').mkdir(parents=True)
        archive_name = 'llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz'
        payload = b'inert engine bytes'
        checksum = hashlib.sha256(payload).hexdigest()
        (scripts / 'release-upstream-recipes.json').write_text(json.dumps({'recipes': [{
            'component': 'llama-server', 'platform': 'linux-arm64',
            'source': {'checksum': 'sha256:' + checksum}}]}))
        # Only the cold producer is inert. Archive admission executes the real
        # recipe's verification branch, which ends before any fetch or build.
        verifier = ROOT / 'scripts/build/build-llama-server-bundle.sh'
        recipe = scripts / 'build/build-llama-server-bundle.sh'
        recipe.write_text('''#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --verify-archive ]]; then
    exec bash "$REAL_ENGINE_VERIFIER" "$@"
fi
printf 'build\\n' >> "$ENGINE_CALLS"
mkdir "$1"
printf '%s' 'inert engine bytes' > "$1/llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz"
printf 'recipe_commit=%s\\n' "$RECIPE_COMMIT" > "$1/build-info.txt"
printf 'fixture ELF and reply receipts\\n' > "$1/elf-verification.txt"
''')
        git = self.bin / 'git'
        git.write_text('''#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  'rev-parse HEAD') echo "$SOURCE_COMMIT" ;;
  'rev-parse HEAD^{tree}') echo "$SOURCE_TREE" ;;
  'status --porcelain=v1 --untracked-files=all') ;;
  *) exit 99 ;;
esac
''')
        git.chmod(0o700)
        temporary = self.root / 'runner-temp'
        temporary.mkdir(mode=0o700)
        bundle = temporary / 'llama-arm64-bundle'
        if cached:
            bundle.mkdir(mode=0o700)
            (bundle / archive_name).write_bytes(payload)
            (bundle / 'build-info.txt').write_text('recipe_commit=original-producer\n')
            (bundle / 'elf-verification.txt').write_text('original producer ELF receipt\n')
        self.engine_env = {**self.env, 'RUNNER_ENVIRONMENT': 'github-hosted',
                           'RUNNER_OS': 'Linux', 'RUNNER_ARCH': 'ARM64',
                           'RUNNER_TEMP': str(temporary), 'GITHUB_ENV': str(self.root / 'env'),
                           'REAL_ENGINE_VERIFIER': str(verifier),
                           'ENGINE_CALLS': str(self.root / 'engine-calls'),
                           'BUILD_CONTAINER_IMAGE': 'pinned-container',
                           'ENGINE_BUILD_TOOLS': 'pinned-tools'}
        self.assert_engine_ok(self.engine_step('bind the source and approved ARM64 engine'))
        for line in (self.root / 'env').read_text().splitlines():
            name, value = line.split('=', 1)
            self.engine_env[name] = value
        return bundle

    def engine_step(self, name, workflow='release-package.yml'):
        return subprocess.run(['bash', '-euo', 'pipefail', '-c',
                               self.workflow_script('engine-llama-arm64', 'name: ' + name, workflow)],
                              cwd=self.root, env=self.engine_env, capture_output=True, text=True,
                              timeout=10)

    def assert_engine_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_release_engine_restore_retains_producer_and_matches_ci_key(self):
        bundle = self.engine_fixture(cached=True)
        original = {path.name: path.read_bytes() for path in bundle.iterdir()}
        self.assert_engine_ok(self.engine_step('bind engine cache to build recipe'))
        release_key = self.output.read_text()
        self.output.unlink()
        self.assert_engine_ok(self.engine_step('bind engine cache to build recipe', 'ci.yml'))
        self.assertEqual(self.output.read_text(), release_key)
        self.assert_engine_ok(self.engine_step('verify accepted ARM64 engine archive'))
        self.assertEqual({path.name: path.read_bytes() for path in bundle.iterdir()}, original)
        self.assertFalse(Path(self.engine_env['ENGINE_CALLS']).exists())

    def test_release_engine_cold_build_is_verified_with_current_recipe_receipt(self):
        bundle = self.engine_fixture(cached=False)
        self.assert_engine_ok(self.engine_step('build and validate pinned ARM64 engine'))
        self.assert_engine_ok(self.engine_step('verify accepted ARM64 engine archive'))
        self.assertEqual((bundle / 'build-info.txt').read_text(), 'recipe_commit=' + self.commit + '\n')
        self.assertEqual(Path(self.engine_env['ENGINE_CALLS']).read_text(), 'build\n')

    def test_release_engine_restore_refuses_wrong_archive_without_rebuilding(self):
        bundle = self.engine_fixture(cached=True)
        receipt = (bundle / 'build-info.txt').read_bytes()
        (bundle / 'llama-b10516-bin-ubuntu22.04-arm64-cpu.tar.gz').write_bytes(b'corrupt cache')
        result = self.engine_step('verify accepted ARM64 engine archive')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('SHA-256 mismatch', result.stderr)
        self.assertEqual((bundle / 'build-info.txt').read_bytes(), receipt)
        self.assertFalse(Path(self.engine_env['ENGINE_CALLS']).exists())

    def test_release_cache_statistics_bind_the_exact_source_and_version_pair(self):
        sccache = self.bin / 'sccache'
        sccache.write_text('''#!/usr/bin/env bash
set -euo pipefail
case "$*" in
  --show-stats) printf 'fixture cache statistics\\n' ;;
  '--show-stats --stats-format=json') printf '{"stats":{"cache_hits":{"counts":{"Rust":2}}}}\\n' ;;
  *) exit 99 ;;
esac
''')
        sccache.chmod(0o700)
        for job, platform in (('mac', 'aarch64-darwin'), ('linux', 'aarch64-linux')):
            with self.subTest(platform=platform):
                root = self.root / job
                root.mkdir(mode=0o700)
                result = subprocess.run(['bash', '-euo', 'pipefail', '-c',
                    self.workflow_script(job, 'name: record compiler cache statistics')],
                    cwd=self.root, env={**self.env, 'RELEASE_ROOT': str(root),
                                       'RELEASE_PLATFORM': platform}, capture_output=True,
                    text=True, timeout=10)
                self.assertEqual(result.returncode, 0, result.stderr)
                record = json.loads((root / 'compiler-cache-stats.json').read_bytes())
                self.assertEqual(record['stats']['cache_hits']['counts']['Rust'], 2)
                self.assertEqual(record['release_input'], {
                    'SOURCE_COMMIT': self.commit, 'SOURCE_TREE': self.tree,
                    'RELEASE_PLATFORM': platform, 'INSTALL_VERSION': '1.2.3', 'UPDATE_VERSION': '1.2.4'})


class SourceUpstreamTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='release-source-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        (self.root / 'scripts').mkdir(mode=0o700)
        for name in ('seed-kubo-cache.sh', 'release-upstream-input.py'):
            shutil.copy2(ROOT / 'scripts' / name, self.root / 'scripts' / name)
        self.cache, self.data = self.root / 'cache', self.root / 'data'
        self.data.mkdir(mode=0o700)
        self.recipes = [self.recipe('kubo', 'kubo', 'ipfs', '1.2.3', 'bin/kubo'),
                        self.recipe('llama-server', 'llama-b10516', 'llama-server',
                                    '0.0.10516', 'libexec/llama.cpp/b10516/' + PLATFORM)]
        self.recipes[1]['binary_path'] = 'llama-server'
        # A consumer Home owns model selection. Unselected model inputs remain
        # absent even when the build recipe list contains an unavailable input.
        self.recipes.append({'component': 'model-fixture', 'platform': '*',
                             'source': {'path': str(self.root / 'absent-model')}})
        self.write_recipes()
        self.manifest = {'external': {'llama-server': {'version': 'b10516',
                         'platforms': {PLATFORM: {}}}}}
        (self.data / 'components.json').write_text(json.dumps(self.manifest))

    def recipe(self, component, root, entrypoint, version, install_path):
        archive = self.root / (component + '.tar.gz')
        with tarfile.open(archive, 'w:gz') as bundle:
            for name, payload, mode in ((entrypoint, b'inert ' + component.encode(), 0o755),
                                        ('lib/fixture.txt', b'fixture library', 0o644)):
                member = tarfile.TarInfo(root + '/' + name)
                member.mode, member.size = mode, len(payload)
                bundle.addfile(member, io.BytesIO(payload))
        licence = self.root / (component + '-LICENSE')
        licence.write_bytes(b'fixture MIT licence\n')
        def source(path, algorithm):
            return {'path': str(path), 'checksum': algorithm + ':' +
                    hashlib.new(algorithm, path.read_bytes()).hexdigest(), 'max_bytes': 16384}
        return {'schema': 'elastos.release-upstream-input/v1', 'component': component,
                'platform': PLATFORM, 'version': version, 'source': source(archive, 'sha512'),
                'format': 'tar.gz', 'root': root, 'entrypoint': entrypoint,
                'extract_path': root + '/' + entrypoint if component == 'kubo' else root,
                'install_path': install_path, 'max_unpacked_bytes': 32768,
                'license': {'spdx_id': 'MIT', 'files': [
                    {'name': 'LICENSE', 'source': source(licence, 'sha256')}]}}

    def write_recipes(self):
        (self.root / 'scripts/release-upstream-recipes.json').write_text(json.dumps({
            'schema': 'elastos.release-upstream-recipes/v1', 'recipes': self.recipes}))

    def seed(self, verify=False, data=None, platform=PLATFORM):
        return subprocess.run(['bash', str(self.root / 'scripts/seed-kubo-cache.sh'),
                               str(self.cache), str(data or self.data), platform,
                               *(['--verify-installed'] if verify else [])],
                              capture_output=True, text=True, env={**os.environ, 'PYTHONDONTWRITEBYTECODE': '1'})

    def source_function(self, name, overrides=None):
        source = (ROOT / 'scripts/setup-source-home.sh').read_text()
        body = source.split(name + '() {', 1)[1]
        lines, python = [], False
        for line in body.splitlines():
            lines.append(line)
            if "<<'PY'" in line:
                python = True
            elif line == 'PY':
                python = False
            elif line == '}' and not python:
                break
        function = name + '() {' + '\n'.join(lines)
        environment = {**os.environ, 'ROOT': str(self.root), 'DATA_DIR': str(self.data),
                       'PLATFORM': PLATFORM, 'SETUP_SOURCE_HOME_UPSTREAM_CACHE': str(self.cache),
                       'PYTHONDONTWRITEBYTECODE': '1'}
        environment.pop('SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER', None)
        environment.pop('SETUP_SOURCE_HOME_INSTALL_KUBO', None)
        environment.update(overrides or {})
        return subprocess.run(['bash', '-c', 'set -euo pipefail\numask 077\n' + function + '\n' + name],
                              env=environment, capture_output=True, text=True)

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_kubo_package_is_licensed_and_reuses_verified_inputs(self):
        self.assert_ok(self.seed())
        self.assertEqual((self.data / 'bin/kubo').read_bytes(), b'inert kubo')
        self.assertEqual(stat.S_IMODE((self.data / 'bin/kubo').stat().st_mode), 0o700)
        capsule = self.data / 'capsules/kubo'
        metadata = json.loads((capsule / 'capsule.json').read_bytes())
        self.assertEqual((metadata['role'], metadata['type']), ('content', 'data'))
        self.assertEqual((capsule / 'LICENSE').read_bytes(), b'fixture MIT licence\n')
        receipt_path = self.data / 'receipts/kubo-build.json'
        receipt = json.loads(receipt_path.read_bytes())
        package = self.cache / 'capsules' / receipt['release_path']
        self.assertEqual(receipt['checksum'], 'sha256:' + hashlib.sha256(package.read_bytes()).hexdigest())
        self.assertEqual(receipt['capsule_metadata']['checksum'], receipt['checksum'])
        self.assertEqual((capsule / '.elastos-artifact-sha256').read_text(), receipt['checksum'] + '\n')
        for source in (self.recipes[0]['source'], self.recipes[0]['license']['files'][0]['source']):
            Path(source['path']).unlink()
        self.assert_ok(self.seed(verify=True))
        self.assertEqual(json.loads(receipt_path.read_bytes()), receipt)
        self.assertFalse((self.data / 'capsules/llama-server').exists())
        self.assertEqual(len(list(self.cache.glob('sha*'))), 2)

    def test_kubo_verification_refuses_changed_native_bytes_and_missing_cache(self):
        self.assert_ok(self.seed())
        binary = self.data / 'bin/kubo'
        binary.write_bytes(b'changed installed bytes')
        result = self.seed(verify=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Installed Kubo differs', result.stderr)
        self.assertEqual(binary.read_bytes(), b'changed installed bytes')
        binary.write_bytes(b'inert kubo')
        algorithm, digest = self.recipes[0]['source']['checksum'].split(':')
        (self.cache / (algorithm + '-' + digest)).unlink()
        # The original local input is still present: verify mode must refuse,
        # rather than silently repopulate its cache from any source.
        self.assertNotEqual(self.seed(verify=True).returncode, 0)
        self.assertFalse((self.cache / (algorithm + '-' + digest)).exists())

    def test_kubo_refuses_corrupt_cache_and_symlink_output(self):
        self.assert_ok(self.seed())
        algorithm, digest = self.recipes[0]['source']['checksum'].split(':')
        (self.cache / (algorithm + '-' + digest)).write_bytes(b'corrupt archive')
        pristine = self.root / 'other-data'
        self.assertNotEqual(self.seed(data=pristine).returncode, 0)
        self.assertFalse((pristine / 'bin/kubo').exists())
        alias = self.root / 'alias'
        alias.symlink_to(self.data)
        self.assertNotEqual(self.seed(data=alias).returncode, 0)

    def test_engine_is_opt_in_and_install_matches_runtime_receipt_contract(self):
        self.assert_ok(self.source_function('install_local_model_engine'))
        self.assertFalse(self.cache.exists())
        self.assertFalse((self.data / 'capsules').exists())
        self.assert_ok(self.source_function('install_local_model_engine', {'SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER': '1'}))
        info = json.loads((self.data / 'components.json').read_bytes())['external']['llama-server']
        bundle = self.data / info['platforms'][PLATFORM]['install_path']
        receipt = json.loads((bundle / '.elastos-engine.json').read_bytes())
        self.assertEqual(receipt['schema'], 'elastos.local-model-engine/v2')
        self.assertEqual(receipt['version'], info['version'])
        self.assertEqual(receipt['archive_sha256'], info['platforms'][PLATFORM]['checksum'])
        files = [p for p in bundle.rglob('*') if p.is_file() and p.name != '.elastos-engine.json']
        expected = sorted(({'path': p.relative_to(bundle).as_posix(), 'type': 'file',
                            'sha256': 'sha256:' + hashlib.sha256(p.read_bytes()).hexdigest()} for p in files),
                          key=lambda row: row['path'])
        self.assertEqual(receipt['entries'], expected)
        self.assertEqual(stat.S_IMODE(bundle.stat().st_mode), 0o500)
        self.assertEqual(stat.S_IMODE((bundle / 'llama-server').stat().st_mode), 0o500)
        self.assertEqual(stat.S_IMODE((bundle / '.elastos-engine.json').stat().st_mode), 0o400)
        self.assertEqual((self.data / 'bin/llama-server').resolve(), bundle / 'llama-server')
        self.assertTrue((self.data / 'capsules/llama-server/LICENSE').is_file())
        self.assertEqual((self.data / 'capsules/llama-server/.elastos-artifact-sha256').read_text(),
                         receipt['archive_sha256'] + '\n')
        self.assert_ok(self.source_function('install_local_model_engine', {'SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER': '1'}))
        self.assertEqual(len(list(self.cache.glob('sha*'))), 2)

    def test_engine_refuses_unrecorded_existing_files_before_link_activation(self):
        bundle = self.data / self.recipes[1]['install_path']
        bundle.mkdir(parents=True, mode=0o700)
        (bundle / 'unexpected').write_bytes(b'unrecorded')
        result = self.source_function('install_local_model_engine', {'SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER': '1'})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('unexpected file', result.stderr)
        self.assertFalse((self.data / 'bin/llama-server').exists())
        self.assertEqual(json.loads((self.data / 'components.json').read_bytes()), self.manifest)

    def test_source_home_kubo_verifies_preseed_and_linux_auto_stays_lazy(self):
        self.assert_ok(self.source_function('install_content_publish_backend'))
        self.assertFalse(self.cache.exists())
        self.assert_ok(self.seed())
        self.assert_ok(self.source_function('install_content_publish_backend'))
        (self.data / 'bin/kubo').write_bytes(b'changed')
        result = self.source_function('install_content_publish_backend')
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Installed Kubo differs', result.stderr)

    def test_source_home_mac_reuses_ci_preseed_cache_without_fetching(self):
        runner = self.root / 'runner-temp'
        self.cache = runner / 'kubo-cache'
        self.recipes[0]['platform'] = 'darwin-arm64'
        self.write_recipes()
        self.assert_ok(self.seed(platform='darwin-arm64'))
        cached = {path.name: (path.stat().st_ino, path.read_bytes())
                  for path in self.cache.glob('sha*')}
        receipt = (self.data / 'receipts/kubo-build.json').read_bytes()
        # Match CI: seed-kubo used RUNNER_TEMP/kubo-cache, then a later shell
        # runs source-home without either explicit cache override. Removing the
        # original inputs proves that verification uses the same pinned cache.
        for source in (self.recipes[0]['source'], self.recipes[0]['license']['files'][0]['source']):
            Path(source['path']).unlink()
        environment = {'PLATFORM': 'darwin-arm64', 'RUNNER_TEMP': str(runner),
                       'SETUP_SOURCE_HOME_UPSTREAM_CACHE': '', 'KUBO_CACHE_DIR': ''}
        self.assert_ok(self.source_function('install_content_publish_backend', environment))
        self.assertFalse((self.root / 'target-build/upstream-cache').exists())
        self.assertEqual((self.data / 'receipts/kubo-build.json').read_bytes(), receipt)
        self.assertEqual({path.name: (path.stat().st_ino, path.read_bytes())
                          for path in self.cache.glob('sha*')}, cached)
        for overrides in ({'SETUP_SOURCE_HOME_UPSTREAM_CACHE': str(self.cache),
                           'KUBO_CACHE_DIR': str(self.root / 'wrong-kubo-cache')},
                          {'KUBO_CACHE_DIR': str(self.cache)}):
            self.assert_ok(self.source_function('install_content_publish_backend', {
                **environment, 'RUNNER_TEMP': str(self.root / 'wrong-runner-temp'), **overrides}))
        self.assertFalse((self.root / 'wrong-runner-temp').exists())
        self.assertFalse((self.root / 'wrong-kubo-cache').exists())
        binary = self.data / 'bin/kubo'
        binary.write_bytes(b'changed installed bytes')
        result = self.source_function('install_content_publish_backend', environment)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('Installed Kubo differs', result.stderr)
        self.assertEqual(binary.read_bytes(), b'changed installed bytes')

    def test_source_home_stamps_native_pins_and_keeps_provider_runtime(self):
        self.assert_ok(self.seed())
        source = {'external': {}, 'profiles': {}}
        for name in ('shell', 'ipfs-provider', 'kubo', 'operator-drive-adapter',
                     'drm-provider', 'rights-provider', 'key-provider', 'decrypt-provider'):
            source['external'][name] = {'install_path': 'bin/' + name,
                'platforms': {PLATFORM: {'install_path': 'bin/' + name}}}
        source['external']['ipfs-provider']['provider_runtime'] = {'role': 'native-fixture'}
        (self.root / 'components.json').write_text(json.dumps(source))
        for name in ('shell', 'ipfs-provider'):
            (self.data / 'bin' / name).write_bytes(('inert ' + name).encode())
        self.assert_ok(self.source_function('stamp_source_home_components_manifest', {
            'SOURCE_HOME_BINARY_NAMES_JSON': '["ipfs-provider"]', 'APP_CAPSULES_JSON': '[]',
            'SOURCE_HOME_KUBO_INSTALLED': '1', 'SOURCE_HOME_LLAMA_INSTALLED': '0'}))
        installed = json.loads((self.data / 'components.json').read_bytes())
        self.assertEqual(installed['external']['ipfs-provider']['provider_runtime'],
                         source['external']['ipfs-provider']['provider_runtime'])
        kubo = installed['external']['kubo']
        self.assertEqual(kubo['platforms'][PLATFORM]['checksum'],
                         'sha256:' + hashlib.sha256(b'inert kubo').hexdigest())
        receipt = json.loads((self.data / 'receipts/kubo-build.json').read_bytes())
        self.assertEqual(kubo['capsule_metadata']['platforms'][PLATFORM]['checksum'], receipt['checksum'])
        self.assertIn('kubo', installed['profiles']['source-home']['components'])

    def test_carrier_fixture_colocates_kubo_capsule_and_starts_unseeded_consumer(self):
        self.assert_ok(self.seed())
        source = (ROOT / 'scripts/local-carrier-setup-smoke.sh').read_text()
        staging = source.split("cache = pathlib.Path(os.environ.get('KUBO_CACHE_DIR'", 1)[1]
        staging = "cache = pathlib.Path(os.environ.get('KUBO_CACHE_DIR'" + staging.split('\nif platform == "linux-arm64":', 1)[0]
        import importlib.util
        spec = importlib.util.spec_from_file_location('upstream_fixture', self.root / 'scripts/release-upstream-input.py')
        upstream = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(upstream)
        artifacts = self.root / 'publisher/artifacts'
        artifacts.mkdir(parents=True, mode=0o700)
        manifest = {'external': {'kubo': {'platforms': {PLATFORM: {'strategy': 'source-build'}}}}}
        cid = 'bafk' + 'a' * 32
        context = {'pathlib': __import__('pathlib'), 'os': os, 'json': json, 'shutil': shutil,
                   'subprocess': subprocess, 'data_dir': self.data, 'artifacts_dir': artifacts,
                   'upstream': upstream, 'manifest': manifest, 'platform': PLATFORM,
                   'platform_info': lambda name: manifest['external'][name]['platforms'][PLATFORM]}
        with mock.patch.dict(os.environ, {'KUBO_CACHE_DIR': str(self.cache)}), \
                mock.patch.object(subprocess, 'run') as initialize, \
                mock.patch.object(subprocess, 'check_output', return_value=cid + '\n') as publish:
            exec(compile(staging, 'local-carrier-kubo-fixture', 'exec'), context)
            self.assertEqual(initialize.call_args.kwargs['env']['IPFS_PATH'], str(self.data / 'ipfs-repo'))
            self.assertEqual(publish.call_args.args[0][-1], str(artifacts / 'kubo-linux-amd64.tar.gz'))
        info = manifest['external']['kubo']['platforms'][PLATFORM]
        metadata = manifest['external']['kubo']['capsule_metadata']['platforms'][PLATFORM]
        self.assertEqual((info['cid'], metadata['cid']), (cid, cid))
        self.assertEqual(info['checksum'], metadata['checksum'])
        self.assertNotIn('strategy', info)
        self.assertEqual(info['checksum'], 'sha256:' + hashlib.sha256((artifacts / info['release_path']).read_bytes()).hexdigest())
        (self.data / 'model-catalog.json').write_text('{}')
        prerequisite = self.root / 'elastos/target/release/localhost-provider'
        prerequisite.parent.mkdir(parents=True)
        prerequisite.write_bytes(b'inert localhost fixture')
        consumer = source.split('PUBLISHER_DATA_DIR="${DATA_DIR}"', 1)[1].split('\nSOURCES_PATH=', 1)[0]
        consumer = 'PUBLISHER_DATA_DIR="${DATA_DIR}"' + consumer
        binary_path = 'cargo_release_binary() {' + source.split('cargo_release_binary() {', 1)[1].split('\n}', 1)[0] + '\n}\n'
        result = subprocess.run(['bash', '-c', 'set -euo pipefail\numask 077\n' + binary_path + consumer],
            env={**os.environ, 'DATA_DIR': str(self.data), 'TEST_ROOT': str(self.root),
                 'REPO_ROOT': str(self.root)}, capture_output=True, text=True)
        self.assert_ok(result)
        data = self.root / 'consumer-data/elastos'
        self.assertEqual((data / 'bin/localhost-provider').read_bytes(), b'inert localhost fixture')
        self.assertFalse((data / 'bin/kubo').exists())
        self.assertFalse((data / 'capsules/kubo').exists())
        self.assertTrue((self.data / 'bin/kubo').is_file())


if __name__ == '__main__':
    unittest.main()
