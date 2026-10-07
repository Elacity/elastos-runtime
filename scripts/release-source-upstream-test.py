#!/usr/bin/env python3
"""Inert source-builder fixtures; all payload and licence inputs are local files."""

import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import stat
import subprocess
import tarfile
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parent.parent
PLATFORM = 'linux-amd64'


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
        self.assertEqual((capsule / '.elastos-artifact-sha256').read_text(),
                         receipt['checksum'].removeprefix('sha256:') + '\n')
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
        marker = self.data / 'capsules/llama-server/.elastos-artifact-sha256'
        self.assertEqual(marker.read_text(), receipt['archive_sha256'].removeprefix('sha256:') + '\n')
        # A Home set up by an earlier release keeps its prefixed receipt.
        marker.parent.chmod(0o700)
        marker.chmod(0o600)
        marker.write_text(receipt['archive_sha256'] + '\n')
        marker.chmod(0o400)
        marker.parent.chmod(0o500)
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
