#!/usr/bin/env python3
"""Browser build and admission fixtures, with inert local sources and mocked tools."""
import copy
import hashlib
import importlib.util
import io
import json
import os
import struct
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent


def module(name):
    spec = importlib.util.spec_from_file_location(name.replace('-', '_'), ROOT / 'scripts' / (name + '.py'))
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value


builder = module('browser-upstream-build')
upstream = module('release-upstream-input')
assets = module('release-upstream-assets')
platform_input = module('release-platform-input')


def crosvm_sources():
    with tarfile.open(ROOT / 'scripts/build/crosvm-lifecycle-source-fixture.tar.gz') as archive:
        provenance = json.load(archive.extractfile('SOURCE.json'))
        assert provenance['revision'] == '2ac141082d171e76a39cdabf98f6e48a377a126b'
        files = {name: archive.extractfile(name).read() for name in provenance['files']}
        for name, data in files.items():
            assert hashlib.sha256(data).hexdigest() == provenance['files'][name]['sha256']
        return files


class BrowserUpstreamBuildTest(unittest.TestCase):
    def test_crosvm_lifecycle_recipe_changes_the_installed_identity(self):
        recipe = next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == 'crosvm')
        component = json.loads((ROOT / 'components.json').read_bytes())['external']['crosvm']
        self.assertEqual(recipe['version'], '0.1.0-2ac141082d17.elastos.1')
        self.assertEqual(component['version'], recipe['version'])
        self.assertEqual(recipe['install_path'], 'libexec/browser/crosvm/' + recipe['version'] + '/linux-arm64')
        self.assertEqual(component['platforms']['linux-arm64']['install_path'], recipe['install_path'])
        self.assertNotEqual(recipe['install_path'], 'libexec/browser/crosvm/0.1.0-2ac141082d17/linux-arm64')

    def test_crosvm_patch_applies_to_pinned_source_and_refuses_changed_context(self):
        recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == 'crosvm'))
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            original = crosvm_sources()
            for name, data in original.items():
                path = root / 'src' / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            builder.apply_crosvm_patch(root / 'src', recipe, root / 'cache', upstream)
            self.assertIn('pub lifecycle_fds:', (root / 'src' / builder.CROSVM_PATCHED_FILES[0]).read_text())
            for name, data in original.items():
                (root / 'src' / name).write_bytes(data)
            changed = root / 'src' / builder.CROSVM_PATCHED_FILES[0]
            changed.write_text(changed.read_text().replace('pub lock: bool,', 'pub incompatible_lock: bool,'))
            before = {name: (root / 'src' / name).read_bytes() for name in original}
            with self.assertRaises(subprocess.CalledProcessError):
                builder.apply_crosvm_patch(root / 'src', recipe, root / 'cache', upstream)
            self.assertEqual(before, {name: (root / 'src' / name).read_bytes() for name in original})
        missing = copy.deepcopy(recipe)
        missing['notices'] = [n for n in missing['notices'] if n['name'] != builder.CROSVM_PATCH]
        with self.assertRaisesRegex(ValueError, 'pinned block lifecycle patch'):
            builder.validate(missing, upstream)

    def test_source_extraction_accepts_git_comment_and_empty_files_only_for_sources(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch)
            source = root / 'source.tar.gz'
            with tarfile.open(source, 'w:gz', format=tarfile.PAX_FORMAT, pax_headers={'comment': 'pinned Git commit'}) as archive:
                item = tarfile.TarInfo('source/empty.c')
                archive.addfile(item, io.BytesIO(b''))
                item = tarfile.TarInfo('source/nonempty.c')
                item.size = 1
                archive.addfile(item, io.BytesIO(b'x'))
                item = tarfile.TarInfo('source/.cargo/config.toml')
                item.size = 1
                archive.addfile(item, io.BytesIO(b'x'))
                # crosvm's pinned tree has recipe fixtures such as "full props.json".
                item = tarfile.TarInfo('source/expected/full props.json')
                item.size = 1
                archive.addfile(item, io.BytesIO(b'x'))
                for number in range(4500):
                    item = tarfile.TarInfo('source/tests/empty' + str(number))
                    item.type = tarfile.DIRTYPE
                    archive.addfile(item)
            extracted = builder.extract(source, 'source', root / 'extracted', upstream)
            self.assertEqual((extracted / 'empty.c').read_bytes(), b'')
            self.assertEqual((extracted / 'nonempty.c').read_bytes(), b'x')
            self.assertEqual((extracted / '.cargo/config.toml').read_bytes(), b'x')
            self.assertEqual((extracted / 'expected/full props.json').read_bytes(), b'x')
            # Installed payloads keep the stricter rule; edge spaces stay refused in sources.
            for name, mode in (('source/expected/full props.json', True), ('source/trailing ', 'build'),
                               ('source/ leading', 'build')):
                with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'unsafe package path'):
                    upstream.relative(name, source_archive=mode)
            with self.assertRaises(ValueError):
                upstream.archive_members(source, 'tar.gz', 'source', 1024)
            with tarfile.open(source, 'w:gz', format=tarfile.PAX_FORMAT, pax_headers={'path': '../foreign'}) as archive:
                item = tarfile.TarInfo('source/empty.c')
                archive.addfile(item, io.BytesIO(b''))
            with self.assertRaises(ValueError):
                builder.extract(source, 'source', root / 'foreign', upstream)
            self.assertFalse((root / 'foreign').exists())

    def test_library_audit_accepts_only_system_runtime_libraries(self):
        builder.audit_libraries('turnserver:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1.0.0)\n', True)
        builder.audit_libraries('statically linked\n', False)
        for output, mac in [('turn:\n\t/opt/homebrew/lib/libssl.3.dylib (version 3)\n', True),
                            ('turn:\n\t@rpath/libevent.dylib (version 1)\n', True),
                            ('libevent.so.2 => /usr/lib/libevent.so.2 (0x01)\n', False),
                            ('libc.so.6 => /lib/libc.so.6 (0x01)\n', False),
                            ('libc.so.6 => /tmp/build/libc.so.6 (0x01)\n', False),
                            ('libssl.so.3 => not found\n', False), ('', False)]:
            with self.subTest(output=output), self.assertRaises(ValueError):
                builder.audit_libraries(output, mac)

    def test_native_build_packages_sources_licenses_and_relocation_audit(self):
        for platform in ('darwin-arm64', 'linux-arm64'):
            with self.subTest(platform=platform):
                self.native_build(platform)

    def test_coturn_build_refuses_a_changed_include_path_marker(self):
        with self.assertRaisesRegex(ValueError, 'coturn include-path marker changed'):
            self.native_build('linux-arm64', b'LIBEVENT_INCLUDE = -I/usr/include/\n')

    def native_build(self, platform, coturn_makefile=b'LIBEVENT_INCLUDE = -I${PREFIX}/include/ -I/usr/local/include/\n'):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes(platform) if r['component'] == 'turnserver'))
            def source(name, data):
                path = root / name
                path.write_bytes(data)
                return {'path': str(path), 'checksum': 'sha256:' + hashlib.sha256(data).hexdigest(), 'max_bytes': len(data)}
            def archive_source(name):
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode='w:gz', format=tarfile.GNU_FORMAT) as archive:
                    member = tarfile.TarInfo(name + '/configure')
                    data = b'inert configure fixture\n'
                    member.size, member.mode = len(data), 0o755
                    archive.addfile(member, io.BytesIO(data))
                    extra = {}
                    if name.startswith('openssl-'):
                        extra['util/mkbuildinf.pl'] = b"my $cflags = join(' ', @ARGV);\nprint $cflags;\n"
                    if name.startswith('coturn-'):
                        extra['Makefile.in'] = coturn_makefile + b'INCFLAGS = -Isrc ${LIBEVENT_INCLUDE}\n'
                    for relative, data in extra.items():
                        member = tarfile.TarInfo(name + '/' + relative)
                        member.size = len(data)
                        archive.addfile(member, io.BytesIO(data))
                return source(name + '.tar.gz', buffer.getvalue())
            recipe['source'] = archive_source(recipe['root'])
            for dependency in recipe['build']['dependencies']:
                dependency['source'] = archive_source(dependency['root'])
            for item in recipe['license']['files']:
                item['source'] = source(item['name'].replace('/', '-'), b'fixture licence: ' + item['name'].encode())
            calls, environments = [], []
            real_run = subprocess.run
            mac = platform.startswith('darwin')
            headers = root / 'uapi'
            for name in ('linux', 'asm-generic', 'asm'):
                (headers / name).mkdir(parents=True)
                (headers / name / 'fixture.h').write_bytes(b'build header fixture')
            def run(args, **kwargs):
                calls.append(args)
                environments.append(kwargs.get("env", {}))
                if args[0] == 'perl':
                    env = kwargs['env']
                    result = real_run(['perl', str(kwargs['cwd'] / 'util/mkbuildinf.pl'), env['CFLAGS']],
                                      env=env, capture_output=True, text=True, check=True)
                    self.assertNotIn(env['ELASTOS_BROWSER_BUILD_ROOT'], result.stdout)
                    self.assertIn('/usr/src/elastos-browser', result.stdout)
                    # OpenSSL's Configure turns -static into no-threads.
                    self.assertNotIn('-static', env['LDFLAGS'].split())
                if args[0] == 'pkg-config':
                    packages = [a for a in args[1:] if not a.startswith('--')]
                    output = '-Ifixture/include' if '--cflags' in args else ' '.join(
                        '-l' + name.removeprefix('lib') for name in packages)
                    return subprocess.CompletedProcess(args, 0, output + '\n', '')
                if args[0] == './configure' and Path(kwargs['cwd']).name == recipe['root']:
                    makefile = (Path(kwargs['cwd']) / 'Makefile.in').read_text()
                    self.assertIn('LIBEVENT_INCLUDE =\n', makefile)
                    self.assertNotIn('/usr/local/include', makefile)
                    if not mac:
                        uapi = Path(kwargs['env']['C_INCLUDE_PATH'])
                        self.assertEqual((uapi / 'linux/fixture.h').read_bytes(), b'build header fixture')
                    # GNU ld resolves static archives in order: libevent_openssl, then OpenSSL.
                    libraries = kwargs['env']['EVENT_LIBS'].split()
                    self.assertLess(libraries.index('-levent_openssl'), libraries.index('-lssl'))
                    self.assertLess(libraries.index('-lssl'), libraries.index('-lcrypto'))
                if args[:2] == ['make', 'install'] and Path(kwargs['cwd']).name == recipe['root']:
                    binary = Path(kwargs['cwd']).parents[1] / 'install/usr/bin/turnserver'
                    binary.parent.mkdir(parents=True)
                    header = bytearray(64)
                    if mac:
                        header[:4] = b"\xcf\xfa\xed\xfe"
                        struct.pack_into('<I', header, 4, 0x100000C)
                        struct.pack_into('<I', header, 12, 2)
                    else:
                        header[:7] = b"\x7fELF\x02\x01\x01"
                        struct.pack_into('<HH', header, 16, 2, 183 if platform == 'linux-arm64' else 62)
                    binary.write_bytes(header)
                    binary.chmod(0o755)
                if args[0] == 'ldd':
                    return subprocess.CompletedProcess(args, 1, '', 'not a dynamic executable\n')
                if args[0] == 'otool':
                    return subprocess.CompletedProcess(args, 0, 'turnserver:\n\t/usr/lib/libSystem.B.dylib (compatibility version 1)\n', '')
                return subprocess.CompletedProcess(args, 0, '', '')
            with patch.object(builder.platform, 'system', return_value='Darwin' if mac else 'Linux'), \
                 patch.object(builder.platform, 'machine', return_value='x86_64' if platform == 'linux-amd64' else 'arm64'), \
                 patch.object(builder.shutil, 'which', return_value='/fixture/build-tool'), \
                 patch.object(builder.subprocess, 'run', side_effect=run), \
                 patch.dict(os.environ, {'ELASTOS_BROWSER_LINUX_UAPI_INCLUDE': str(headers)}), \
                 patch.object(upstream, 'builder_module', return_value=builder):
                receipt = upstream.package(recipe, root / 'cache', root / 'output')
            self.assertFalse((root / "cache" / ("turnserver-built-" + platform + ".tar.gz")).exists())
            path = root / 'output' / receipt['release_path']
            platform_input.check_upstream_archive(path, recipe, receipt, {'linux-amd64': 'x86_64-linux', 'linux-arm64': 'aarch64-linux', 'darwin-arm64': 'aarch64-darwin'}[platform])
            with tarfile.open(path) as archive:
                names = archive.getnames()
                for name in ['bin/turnserver', 'LIBRARY-AUDIT.txt', 'licenses/openssl.txt', 'licenses/libevent.txt',
                             'sources/coturn-4.13.1.tar.gz', 'sources/openssl-3.6.3.tar.gz',
                             'sources/libevent-release-2.1.13-stable.tar.gz', 'sources/browser-upstream-build.py']:
                    self.assertIn(recipe['root'] + '/' + name, names)
                self.assertEqual(recipe['root'] + '/sources/linux-uapi.tar.gz' in names, not mac)
            if not mac:
                self.assertTrue(any(e.get('CC') == 'musl-gcc' and ' -static' in e.get('LDFLAGS', '') for e in environments))
            self.assertTrue(any('no-shared' in args for args in calls))
            self.assertTrue(any('--disable-shared' in args for args in calls))
            self.assertTrue(any('--enable-openssl' in args for args in calls))
            self.assertTrue(any(e.get('PKG_CONFIG_SYSROOT_DIR') and e.get('TURN_DISABLE_RPATH') == '1' for e in environments))
            self.assertTrue(any('relocated/bin/turnserver' in args[0] for args in calls))
            self.assertFalse(any(args[0] in ('curl', 'wget') for args in calls))

    def test_asset_json_stdout_carries_only_json_while_builds_write_output(self):
        # Native configure/make inherit file descriptors; their output must not
        # enter the JSON that publish-release.sh captures from stdout.
        program = ('import importlib.util, subprocess, sys\n'
                   'spec = importlib.util.spec_from_file_location("assets", sys.argv[1])\n'
                   'assets = importlib.util.module_from_spec(spec); spec.loader.exec_module(assets)\n'
                   'def prepare(*args):\n'
                   '    subprocess.run(["sh", "-c", "echo checking for gcc... yes"], check=True)\n'
                   '    print("python build note")\n'
                   '    return {"external": {"turnserver": {}}}\n'
                   'assets.prepare = prepare\n'
                   'sys.argv = ["release-upstream-assets.py", "--platform", "darwin-arm64", "--cache", "c", "--output", "o"]\n'
                   'assets.main()\n')
        result = subprocess.run([sys.executable, '-c', program, str(ROOT / 'scripts/release-upstream-assets.py')],
                                capture_output=True, text=True, check=True)
        self.assertEqual(json.loads(result.stdout), {'external': {'turnserver': {}}})
        self.assertIn('checking for gcc... yes', result.stderr)
        self.assertIn('python build note', result.stderr)

    def test_official_node_archives_pin_each_platform_and_include_license(self):
        inventory = json.loads(assets.RECIPE_FILE.read_text())
        recipes = [r for r in inventory["recipes"] if r["component"] == "node"]
        self.assertEqual({r["platform"] for r in recipes}, {"darwin-arm64", "linux-arm64"})
        self.assertEqual(len({r["source"]["checksum"] for r in recipes}), 2)
        for recipe in recipes:
            upstream.validate_recipe(recipe)
            self.assertTrue(recipe["source"]["url"].startswith("https://nodejs.org/download/release/v24.21.0/"))
            self.assertEqual(recipe["license"]["files"], [{"name": "LICENSE", "from_archive": "LICENSE"}])
            for field in ("checksum", "license"):
                bad = copy.deepcopy(recipe)
                if field == "checksum":
                    bad["source"].pop("checksum")
                else:
                    bad["license"]["files"] = []
                with self.assertRaises(ValueError):
                    upstream.validate_recipe(bad)

    def test_pinned_archive_license_is_preserved_and_required(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            for present in (True, False):
                recipe = copy.deepcopy(next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == 'node'))
                archive_path = root / ('node-' + str(present) + '.tar.gz')
                with tarfile.open(archive_path, 'w:gz', format=tarfile.GNU_FORMAT) as archive:
                    for name, data, mode in [('bin/node', b'inert node fixture', 0o755),
                                             ('lib/node_modules/@unused/.npmrc', b'', 0o644),
                                             *([('LICENSE', b'fixture MIT licence', 0o644)] if present else [])]:
                        member = tarfile.TarInfo(recipe['root'] + '/' + name)
                        member.size, member.mode = len(data), mode
                        archive.addfile(member, io.BytesIO(data))
                recipe['source'] = {'path': str(archive_path), 'checksum': 'sha256:' + upstream.digest(archive_path),
                                    'max_bytes': archive_path.stat().st_size}
                if not present:
                    with self.assertRaisesRegex(ValueError, 'licence is missing'):
                        upstream.package(recipe, root / 'cache', root / 'missing')
                    continue
                receipt = upstream.package(recipe, root / 'cache', root / 'output')
                with tarfile.open(root / 'output' / receipt['release_path']) as archive:
                    self.assertFalse(any('node_modules' in name for name in archive.getnames()))
                    self.assertEqual(archive.extractfile(recipe['root'] + '/LICENSE').read(), b'fixture MIT licence')
                    provenance = json.load(archive.extractfile(recipe['root'] + '/PROVENANCE.json'))
                    notice = provenance['notices'][0]['source']
                    self.assertEqual(notice['checksum'], recipe['source']['checksum'])
                    self.assertEqual(notice['archive_member'], recipe['root'] + '/LICENSE')

    def test_recipe_pins_and_licenses_are_required_before_build(self):
        recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('darwin-arm64') if r['component'] == 'turnserver'))
        upstream.validate_recipe(recipe)
        for key in ('source', 'license', 'dependency'):
            bad = copy.deepcopy(recipe)
            if key == 'source':
                bad['source']['checksum'] = 'sha256:bad'
            elif key == 'license':
                bad['license']['files'] = []
            else:
                bad['build']['dependencies'][0]['source'].pop('checksum')
            with self.assertRaises(ValueError):
                upstream.validate_recipe(bad)

    def test_python_notices_preserve_payload_and_refuse_incomplete_distribution(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('darwin-arm64') if r['component'] == 'python3'))
            supplement = root / 'supplement.txt'
            supplement.write_bytes(b'pinned supplemental distribution notice')
            for item in recipe['license']['files']:
                if item['name'].startswith('licenses/'):
                    item['source'] = {'path':str(supplement), 'checksum':'sha256:'+upstream.digest(supplement), 'max_bytes':supplement.stat().st_size}
            payload = root / 'python.tar.gz'
            data = b'unchanged executable bytes'
            with tarfile.open(payload, 'w:gz') as archive:
                item = tarfile.TarInfo('python/bin/python3.12')
                item.size, item.mode = len(data), 0o755
                archive.addfile(item, io.BytesIO(data))
                # ARM64 has a large, case-colliding terminal database.
                for number in range(4500):
                    item = tarfile.TarInfo('python/share/terminfo/a/alias' + str(number))
                    item.type, item.linkname = tarfile.SYMTYPE, '../../../bin/python3.12'
                    archive.addfile(item)
                item = tarfile.TarInfo('python/share/terminfo/A/ALIAS0')
                item.type, item.linkname = tarfile.SYMTYPE, '../../../bin/python3.12'
                archive.addfile(item)
            base = {'target_triple': 'aarch64-apple-darwin', 'python_version': recipe['version'],
                    'build_info': {'extensions': [{'license_paths': ['licenses/openssl.txt', 'licenses/LICENSE.zlib-ng.txt']}]}}
            for case in ('complete', 'missing', 'wrong-target', 'duplicate'):
                metadata = copy.deepcopy(base)
                if case == 'wrong-target':
                    metadata['target_triple'] = 'aarch64-unknown-linux-gnu'
                buffer = io.BytesIO()
                with tarfile.open(fileobj=buffer, mode='w') as archive:
                    entries = [('python/PYTHON.json', json.dumps(metadata).encode())]
                    if case != 'missing':
                        entries.append(('python/licenses/openssl.txt', b'pinned distribution notice'))
                    if case == 'duplicate':
                        entries.append(entries[0])
                    for name, value in entries:
                        item = tarfile.TarInfo(name)
                        item.size = len(value)
                        archive.addfile(item, io.BytesIO(value))
                class Process:
                    def __init__(self, *args, **kwargs):
                        self.stdout = io.BytesIO(buffer.getvalue())
                    def __enter__(self):
                        return self
                    def __exit__(self, *args):
                        self.stdout.close()
                    def wait(self):
                        return 0
                cache = root / case
                cache.mkdir()
                with self.subTest(case=case), patch.object(upstream, 'cached_input', side_effect=lambda source, *args: Path(source['path']) if 'path' in source else root / 'full.zst'), \
                     patch.object(builder.subprocess, 'Popen', Process):
                    if case != 'complete':
                        with self.assertRaises(ValueError):
                            builder.package_python_licenses(recipe, payload, cache, upstream)
                        self.assertEqual(list(cache.iterdir()), [])
                        continue
                    output = builder.package_python_licenses(recipe, payload, cache, upstream)
                    files = upstream.archive_members(output, 'tar.gz', 'python', recipe['max_unpacked_bytes'],
                                                     source_archive=True)
                    self.assertFalse(any(name.startswith('python/share/terminfo/') for name in files))
                    with tarfile.open(output) as archive:
                        self.assertEqual(archive.extractfile('python/bin/python3.12').read(), data)
                        self.assertEqual(archive.extractfile('python/licenses/openssl.txt').read(), b'pinned distribution notice')
                        self.assertEqual(json.load(archive.extractfile('python/PYTHON-BUILD.json')), base)

    def test_native_browser_build_refuses_wrong_host_before_compilation(self):
        for component in ('debugfs', 'crosvm', 'vhost-device-vsock'):
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == component))
            upstream.validate_recipe(recipe)
            with self.subTest(component=component), patch.object(builder.platform, 'system', return_value='Darwin'), \
                 patch.object(builder.platform, 'machine', return_value='arm64'), patch.object(builder.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'native platform runner'):
                    builder.build(recipe, Path('unused'), Path('unused'), upstream)
                run.assert_not_called()

    def test_vsock_recipe_pins_sources_licenses_platform_and_profile(self):
        recipes = assets.selected_recipes('linux-arm64')
        recipe = next(r for r in recipes if r['component'] == 'vhost-device-vsock')
        commit = '4fe41e353a005b1b7163f3f38efde3523f3b160c'
        self.assertEqual(recipe['source']['url'], 'https://codeload.github.com/rust-vmm/vhost-device/tar.gz/' + commit)
        self.assertEqual(recipe['source']['checksum'], 'sha256:be673089defc9c9af5c873f0d71fea655ca512b5cf9e251fb9867c1ff5008b5e')
        self.assertEqual(recipe['version'], '0.3.0')
        self.assertEqual(recipe['root'], 'vhost-device-' + commit)
        self.assertEqual(recipe['build'], {'kind': 'vhost-device-vsock-static-v1', 'dependencies': []})
        self.assertEqual({f['name']: f['source']['checksum'] for f in recipe['license']['files']}, {
            'LICENSE': 'sha256:cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
            'LICENSE-APACHE': 'sha256:cfc7749b96f63bd31c3c42b5c471bf756814053e847c10f3eb003417bc523d30',
            'LICENSE-BSD-3-Clause': 'sha256:a408ca693a3359c5d56de80ee73db9b62ca4a179adb12e131230c10e915b05f2'})
        for item in recipe['license']['files']:
            self.assertIn('/' + commit + '/', item['source']['url'])
        manifest = json.loads((ROOT / 'components.json').read_text())
        component = manifest['external']['vhost-device-vsock']
        self.assertEqual(set(component['platforms']), {'linux-arm64'})
        self.assertEqual(component['version'], recipe['version'])
        for field in ('extract_path', 'install_path', 'binary_path'):
            self.assertEqual(component['platforms']['linux-arm64'][field], recipe[field])
        self.assertEqual(manifest['profiles']['browser-host']['components'].count('vhost-device-vsock'), 1)
        for target in ('darwin-arm64', 'linux-amd64'):
            self.assertNotIn('vhost-device-vsock', [r['component'] for r in assets.selected_recipes(target)])
        actual = 'sha256:' + hashlib.sha256((ROOT / 'scripts/browser-upstream-build.py').read_bytes()).hexdigest()
        for entry in json.loads(assets.RECIPE_FILE.read_text())['recipes']:
            for notice in entry.get('notices', []):
                if notice.get('source', {}).get('path') == 'scripts/browser-upstream-build.py':
                    self.assertEqual(notice['source']['checksum'], actual)
        upstream.validate_recipe(assets.resolved_recipe(recipe))

    def test_vsock_refuses_wrong_recipe_and_native_runner_before_tools(self):
        recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64')
                                             if r['component'] == 'vhost-device-vsock'))
        for system, machine in [('Darwin', 'arm64'), ('Windows', 'arm64'), ('Linux', 'x86_64')]:
            with self.subTest(system=system, machine=machine), \
                    patch.object(builder.platform, 'system', return_value=system), \
                    patch.object(builder.platform, 'machine', return_value=machine), \
                    patch.object(builder.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'native platform runner'):
                    builder.build(recipe, Path('unused'), Path('unused'), upstream)
                run.assert_not_called()
        for change in ({'platform': 'linux-amd64'}, {'platform': 'darwin-arm64'},
                       {'component': 'crosvm'}, {'license': {'files': []}}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                builder.validate({**recipe, **change}, upstream)

    def test_vsock_build_admits_static_binary_and_retains_locked_source_closure(self):
        self.vsock_build()

    def test_vsock_build_refuses_dynamic_linkage_and_bad_relocation(self):
        with self.assertRaisesRegex(ValueError, 'statically link'):
            self.vsock_build(linkage='libc.so.6 => /lib/libc.so.6 (0x01)\n')
        with self.assertRaisesRegex(ValueError, 'relocation probe failed'):
            self.vsock_build(probe_status=1)

    def vsock_build(self, linkage='not a dynamic executable\n', probe_status=0):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64')
                                                 if r['component'] == 'vhost-device-vsock'))
            source = root / 'source.tar.gz'
            lock = b'locked vsock dependency fixture\n'
            files = {'Cargo.lock': lock, 'Cargo.toml': b'[workspace]\nmembers=["vhost-device-vsock"]\n',
                     '.cargo/config.toml': b'[alias]\nxtask="run --package xtask --"\n',
                     'vhost-device-vsock/Cargo.toml': b'[features]\ndefault=["backend_vsock"]\n'}
            with tarfile.open(source, 'w:gz', format=tarfile.GNU_FORMAT) as archive:
                for relative, data in files.items():
                    item = tarfile.TarInfo(recipe['root'] + '/' + relative)
                    item.size = len(data)
                    archive.addfile(item, io.BytesIO(data))
            recipe['source'] = {'path': str(source), 'checksum': 'sha256:' + upstream.digest(source),
                                'max_bytes': source.stat().st_size}
            for item in recipe['license']['files']:
                file = root / item['name']
                file.write_bytes(b'fixture licence ' + item['name'].encode())
                item['source'] = {'path': str(file), 'checksum': 'sha256:' + upstream.digest(file),
                                  'max_bytes': file.stat().st_size}
            calls = []
            ambient = {key: 'foreign-fixture' for key in ('CARGO_ENCODED_RUSTFLAGS', 'CARGO_TARGET_DIR',
                       'CARGO_BUILD_BUILD_DIR', 'RUSTC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER',
                       'CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_RUSTFLAGS')}
            ambient['RUSTFLAGS'] = 'foreign-fixture'
            def run(args, **kwargs):
                calls.append(args)
                if args[:2] == ['cargo', '+1.91.0']:
                    env = kwargs['env']
                    self.assertTrue(all(key not in env for key in ambient if key != 'RUSTFLAGS'))
                    self.assertIn('target-feature=+crt-static', env['RUSTFLAGS'])
                    self.assertNotIn('foreign-fixture', env['RUSTFLAGS'])
                    self.assertEqual(env['CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER'], 'musl-gcc')
                    self.assertEqual(env['CC_aarch64_unknown_linux_musl'], 'musl-gcc')
                if args[:3] == ['cargo', '+1.91.0', 'vendor']:
                    self.assertEqual(args[3:], ['--locked', '--versioned-dirs', 'registry-vendor'])
                    vendor = kwargs['cwd'] / 'registry-vendor/fixture-1.0.0'
                    vendor.mkdir(parents=True)
                    (vendor / 'LICENSE').write_bytes(b'locked dependency licence')
                    (vendor / 'lib.rs').write_bytes(b'locked dependency source')
                    return subprocess.CompletedProcess(args, 0,
                        '[source.crates-io]\nreplace-with="vendored-sources"\n'
                        '[source.vendored-sources]\ndirectory="registry-vendor"\n', '')
                if args[:3] == ['cargo', '+1.91.0', 'build']:
                    self.assertEqual(args[3:-2], ['--locked', '--offline', '--release', '--target',
                        'aarch64-unknown-linux-musl', '-p', 'vhost-device-vsock'])
                    src = kwargs['cwd']
                    self.assertEqual((src / 'Cargo.lock').read_bytes(), lock)
                    config = (src / '.cargo/config.toml').read_text()
                    self.assertIn('[alias]', config)
                    self.assertIn('directory="registry-vendor"', config)
                    binary = src / 'target/aarch64-unknown-linux-musl/release/vhost-device-vsock'
                    binary.parent.mkdir(parents=True)
                    header = bytearray(64)
                    header[:7] = b'\x7fELF\x02\x01\x01'
                    struct.pack_into('<HH', header, 16, 2, 183)
                    binary.write_bytes(header)
                    binary.chmod(0o755)
                    (binary.parent / 'deps').mkdir()
                    os.link(binary, binary.parent / 'deps/vhost-device-vsock-fixture')
                if args[0] == 'ldd':
                    return subprocess.CompletedProcess(args, 1, '', linkage)
                if args[-1] == '--help':
                    self.assertEqual(kwargs['env']['PATH'], '')
                    self.assertEqual(Path(args[0]).name, 'vhost-device-vsock')
                    return subprocess.CompletedProcess(args, probe_status, 'vhost-device-vsock help\n', '')
                return subprocess.CompletedProcess(args, 0, '', '')
            with patch.object(builder.platform, 'system', return_value='Linux'), \
                    patch.object(builder.platform, 'machine', return_value='aarch64'), \
                    patch.dict(os.environ, ambient), patch.object(builder.subprocess, 'run', side_effect=run), \
                    patch.object(builder, 'linux_uapi_headers') as uapi, \
                    patch.object(upstream, 'builder_module', return_value=builder):
                receipt = upstream.package(recipe, root / 'cache', root / 'output')
                uapi.assert_not_called()
            output = root / 'output' / receipt['release_path']
            platform_input.check_upstream_archive(output, recipe, receipt, 'aarch64-linux')
            self.assertEqual([args[2] for args in calls if args[0] == 'cargo'], ['vendor', 'build'])
            with tarfile.open(output) as archive:
                prefix = recipe['root']
                self.assertEqual(archive.extractfile(prefix + '/sources/Cargo.lock').read(), lock)
                self.assertEqual(archive.extractfile(prefix + '/sources/' + prefix + '.tar.gz').read(), source.read_bytes())
                for name in ('LICENSE', 'LICENSE-APACHE', 'LICENSE-BSD-3-Clause'):
                    self.assertEqual(archive.extractfile(prefix + '/' + name).read(), b'fixture licence ' + name.encode())
                with tarfile.open(fileobj=archive.extractfile(prefix + '/sources/cargo-vendor.tar.gz'), mode='r:gz') as vendor:
                    self.assertEqual(vendor.extractfile('registry-vendor/fixture-1.0.0/LICENSE').read(), b'locked dependency licence')
                    self.assertEqual(vendor.extractfile('registry-vendor/fixture-1.0.0/lib.rs').read(), b'locked dependency source')
                    self.assertTrue(all(m.uid == m.gid == 0 and m.uname == m.gname == '' for m in vendor.getmembers()))

    def test_crosvm_preserves_workspace_and_uses_pinned_static_policy_inputs(self):
        with tempfile.TemporaryDirectory() as scratch:
            root = Path(scratch).resolve()
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == 'crosvm'))
            def source(name, files):
                path = root / (name + '.tar.gz')
                with tarfile.open(path, 'w:gz', format=tarfile.GNU_FORMAT) as archive:
                    for relative, data in files.items():
                        item = tarfile.TarInfo(name + '/' + relative)
                        item.size = len(data)
                        archive.addfile(item, io.BytesIO(data))
                return {'path':str(path), 'checksum':'sha256:'+upstream.digest(path), 'max_bytes':path.stat().st_size}
            recipe['source'] = source(recipe['root'], {
                **crosvm_sources(),
                'Cargo.lock': b'pinned lock fixture',
                'vendor/generic/crypto/Cargo.toml': b'workspace path crate'})
            recipe['build']['dependencies'][0]['source'] = source(recipe['build']['dependencies'][0]['root'], {
                'rust/minijail-sys/build.rs': b'.arg(&jobs)\n            .status()?;',
                'tools/compile_seccomp_policy.py': b'pinned compiler fixture'})
            recipe['build']['dependencies'][1]['source'] = source('libcap-2.78', {'libcap/Makefile': b'fixture'})
            headers = root / 'uapi'
            for name in ('linux', 'asm-generic', 'asm'):
                (headers / name).mkdir(parents=True)
                (headers / name / 'fixture.h').write_bytes(b'build header fixture')
            for item in recipe['license']['files']:
                file = root / item['name'].replace('/', '-')
                file.write_bytes(b'fixture licence ' + item['name'].encode())
                item['source'] = {'path':str(file), 'checksum':'sha256:'+upstream.digest(file), 'max_bytes':file.stat().st_size}
            calls = []
            real_run = subprocess.run
            last_build = {}
            fail_lifecycle = False
            def run(args, **kwargs):
                calls.append(args)
                if args[:2] == ['git', 'apply']:
                    return real_run(args, **kwargs)
                if args[:3] == ['cargo', '+1.91.0', 'vendor']:
                    directory = kwargs['cwd'] / args[-1]
                    if directory.exists():
                        import shutil
                        shutil.rmtree(directory)
                    directory.mkdir()
                    (directory / 'registry-crate.txt').write_bytes(b'locked registry fixture')
                    return subprocess.CompletedProcess(args, 0, '[source.vendored-sources]\ndirectory="registry-vendor"\n', '')
                if args[:3] == ['cargo', '+1.91.0', 'build']:
                    src, env = kwargs['cwd'], kwargs['env']
                    last_build.update(cwd=src, env=env.copy(), jobs=args[args.index('-j') + 1])
                    self.assertEqual((src / 'vendor/generic/crypto/Cargo.toml').read_bytes(), b'workspace path crate')
                    self.assertNotIn('MINIJAIL_DEFAULT_RET_LOG', env)
                    self.assertNotIn('MINIJAIL_DO_NOT_BUILD', env)
                    self.assertEqual(env['MINIJAIL_DIR'], str(src / 'third_party/minijail'))
                    self.assertIn('CC_STATIC_LIBRARY(libminijail.pic.a)', (src / 'third_party/minijail/rust/minijail-sys/build.rs').read_text())
                    self.assertIn('--offline', args)
                    self.assertIn('let _lifecycle_fds = lifecycle_fds;',
                                  (src / 'devices/device_virtio_block/src/asynchronous.rs').read_text())
                    binary = src / 'target/aarch64-unknown-linux-musl/release/crosvm'
                    binary.parent.mkdir(parents=True)
                    header = bytearray(64)
                    header[:7] = b'\x7fELF\x02\x01\x01'
                    struct.pack_into('<HH', header, 16, 2, 183)
                    binary.write_bytes(header)
                    binary.chmod(0o755)
                    # Cargo hard-links each release binary into deps/.
                    (binary.parent / 'deps').mkdir()
                    os.link(binary, binary.parent / 'deps/crosvm-0123456789abcdef')
                if args[:3] == ['cargo', '+1.91.0', 'test']:
                    self.assertEqual(args, ['cargo', '+1.91.0', 'test', '--locked', '--offline', '--release',
                        '--target', 'aarch64-unknown-linux-musl', '-p', 'device_virtio_block', '--lib',
                        'lifecycle_', '-j', last_build['jobs'], '--', '--nocapture'])
                    self.assertEqual(kwargs['cwd'], last_build['cwd'])
                    self.assertEqual(kwargs['env'], last_build['env'])
                    self.assertTrue(kwargs['check'])
                    if fail_lifecycle:
                        raise subprocess.CalledProcessError(101, args, stderr='lifecycle test failed')
                return subprocess.CompletedProcess(args, 0, 'crosvm fixture\n' if args[-1] == 'version' else '',
                                                   'not a dynamic executable\n' if args[0] == 'ldd' else '')
            with patch.object(builder.platform, 'system', return_value='Linux'), \
                 patch.object(builder.platform, 'machine', return_value='aarch64'), \
                 patch.object(builder.shutil, 'which', return_value=None), \
                 patch.dict(os.environ, {'MINIJAIL_DEFAULT_RET_LOG':'1', 'MINIJAIL_DO_NOT_BUILD':'1',
                                         'ELASTOS_BROWSER_LINUX_UAPI_INCLUDE':str(headers)}), \
                 patch.object(builder.subprocess, 'run', side_effect=run), \
                 patch.object(upstream, 'builder_module', return_value=builder):
                receipt = upstream.package(recipe, root / 'cache', root / 'output')
                self.assertEqual([args[2] for args in calls if args[0] == 'cargo'], ['vendor', 'build', 'test'])
                fail_lifecycle = True
                calls.clear()
                with self.assertRaises(subprocess.CalledProcessError):
                    upstream.package(recipe, root / 'failure-cache', root / 'failure-output')
                self.assertFalse((root / 'failure-cache/crosvm-built-linux-arm64.tar.gz').exists())
                self.assertFalse(list((root / 'failure-output').glob('*.tar.gz')))
                self.assertFalse(any(args[0] == 'ldd' for args in calls))
            platform_input.check_upstream_archive(root / 'output' / receipt['release_path'], recipe, receipt, 'aarch64-linux')
            self.assertTrue(any('libcap.a' in call and 'SHARED=no' in call for call in calls))
            with tarfile.open(root / 'output' / receipt['release_path']) as archive:
                vendor = archive.extractfile(recipe['root'] + '/sources/cargo-vendor.tar.gz')
                with tarfile.open(fileobj=vendor, mode='r:gz') as locked:
                    self.assertEqual(locked.extractfile('registry-vendor/registry-crate.txt').read(), b'locked registry fixture')
                    self.assertTrue(all(m.uid == m.gid == 0 and m.uname == m.gname == '' for m in locked.getmembers()))
                with tarfile.open(fileobj=archive.extractfile(recipe['root'] + '/sources/linux-uapi.tar.gz'), mode='r:gz') as uapi:
                    self.assertEqual(uapi.extractfile('linux-uapi/linux/fixture.h').read(), b'build header fixture')
                patch_bytes = archive.extractfile(recipe['root'] + '/' + builder.CROSVM_PATCH).read()
                self.assertEqual(patch_bytes, (ROOT / 'scripts/build/crosvm-lifecycle-fds.patch').read_bytes())
                with tarfile.open(fileobj=archive.extractfile(recipe['root'] + '/sources/patched-source-overlay.tar.gz'), mode='r:gz') as overlay:
                    provenance = json.load(overlay.extractfile(recipe['root'] + '/PATCHED-SOURCE.json'))
                    self.assertEqual(provenance['source'], recipe['source']['checksum'])
                    self.assertEqual(provenance['patch'], 'sha256:' + hashlib.sha256(patch_bytes).hexdigest())
                    for name, checksum in provenance['files'].items():
                        data = overlay.extractfile(recipe['root'] + '/' + name).read()
                        self.assertEqual('sha256:' + hashlib.sha256(data).hexdigest(), checksum)
                    self.assertIn(b'let _lifecycle_fds = lifecycle_fds;', overlay.extractfile(
                        recipe['root'] + '/devices/device_virtio_block/src/asynchronous.rs').read())
            # The same install/version name cannot admit the old producer recipe.
            old_recipe = copy.deepcopy(recipe)
            old_recipe['notices'] = [n for n in old_recipe['notices'] if n['name'] != builder.CROSVM_PATCH]
            with self.assertRaisesRegex(ValueError, 'provenance differs'):
                platform_input.check_upstream_archive(root / 'output' / receipt['release_path'], old_recipe, receipt, 'aarch64-linux')


if __name__ == '__main__':
    unittest.main()
