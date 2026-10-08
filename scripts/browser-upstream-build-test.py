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


class BrowserUpstreamBuildTest(unittest.TestCase):
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
                for number in range(4500):
                    item = tarfile.TarInfo('source/tests/empty' + str(number))
                    item.type = tarfile.DIRTYPE
                    archive.addfile(item)
            extracted = builder.extract(source, 'source', root / 'extracted', upstream)
            self.assertEqual((extracted / 'empty.c').read_bytes(), b'')
            self.assertEqual((extracted / 'nonempty.c').read_bytes(), b'x')
            self.assertEqual((extracted / '.cargo/config.toml').read_bytes(), b'x')
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
        for platform in ('darwin-arm64', 'linux-amd64', 'linux-arm64'):
            with self.subTest(platform=platform):
                self.native_build(platform)

    def native_build(self, platform):
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
                    if name.startswith('openssl-'):
                        member = tarfile.TarInfo(name + '/util/mkbuildinf.pl')
                        data = b"my $cflags = join(' ', @ARGV);\nprint $cflags;\n"
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
            def run(args, **kwargs):
                calls.append(args)
                environments.append(kwargs.get("env", {}))
                if args[0] == 'perl':
                    env = kwargs['env']
                    result = real_run(['perl', str(kwargs['cwd'] / 'util/mkbuildinf.pl'), env['CFLAGS']],
                                      env=env, capture_output=True, text=True, check=True)
                    self.assertNotIn(env['ELASTOS_BROWSER_BUILD_ROOT'], result.stdout)
                    self.assertIn('/usr/src/elastos-browser', result.stdout)
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
            if not mac:
                self.assertTrue(any(e.get('CC') == 'musl-gcc' and ' -static' in e.get('LDFLAGS', '') for e in environments))
            self.assertTrue(any('no-shared' in args for args in calls))
            self.assertTrue(any('--disable-shared' in args for args in calls))
            self.assertTrue(any('--enable-openssl' in args for args in calls))
            self.assertTrue(any(e.get('PKG_CONFIG_SYSROOT_DIR') and e.get('TURN_DISABLE_RPATH') == '1' for e in environments))
            self.assertTrue(any('relocated/bin/turnserver' in args[0] for args in calls))
            self.assertFalse(any(args[0] in ('curl', 'wget') for args in calls))

    def test_official_node_archives_pin_each_platform_and_include_license(self):
        inventory = json.loads(assets.RECIPE_FILE.read_text())
        recipes = [r for r in inventory["recipes"] if r["component"] == "node"]
        self.assertEqual({r["platform"] for r in recipes}, {"darwin-arm64", "linux-amd64", "linux-arm64"})
        self.assertEqual(len({r["source"]["checksum"] for r in recipes}), 3)
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
                recipe = copy.deepcopy(next(r for r in assets.selected_recipes('linux-amd64') if r['component'] == 'node'))
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
        for component in ('debugfs', 'crosvm'):
            recipe = assets.resolved_recipe(next(r for r in assets.selected_recipes('linux-arm64') if r['component'] == component))
            upstream.validate_recipe(recipe)
            with self.subTest(component=component), patch.object(builder.platform, 'system', return_value='Darwin'), \
                 patch.object(builder.platform, 'machine', return_value='arm64'), patch.object(builder.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'native platform runner'):
                    builder.build(recipe, Path('unused'), Path('unused'), upstream)
                run.assert_not_called()

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
            def run(args, **kwargs):
                calls.append(args)
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
                    self.assertEqual((src / 'vendor/generic/crypto/Cargo.toml').read_bytes(), b'workspace path crate')
                    self.assertNotIn('MINIJAIL_DEFAULT_RET_LOG', env)
                    self.assertNotIn('MINIJAIL_DO_NOT_BUILD', env)
                    self.assertEqual(env['MINIJAIL_DIR'], str(src / 'third_party/minijail'))
                    self.assertIn('CC_STATIC_LIBRARY(libminijail.pic.a)', (src / 'third_party/minijail/rust/minijail-sys/build.rs').read_text())
                    self.assertIn('--offline', args)
                    binary = src / 'target/aarch64-unknown-linux-musl/release/crosvm'
                    binary.parent.mkdir(parents=True)
                    header = bytearray(64)
                    header[:7] = b'\x7fELF\x02\x01\x01'
                    struct.pack_into('<HH', header, 16, 2, 183)
                    binary.write_bytes(header)
                    binary.chmod(0o755)
                return subprocess.CompletedProcess(args, 0, 'crosvm fixture\n' if args[-1] == '--version' else '',
                                                   'not a dynamic executable\n' if args[0] == 'ldd' else '')
            with patch.object(builder.platform, 'system', return_value='Linux'), \
                 patch.object(builder.platform, 'machine', return_value='aarch64'), \
                 patch.object(builder.shutil, 'which', return_value=None), \
                 patch.dict(os.environ, {'MINIJAIL_DEFAULT_RET_LOG':'1', 'MINIJAIL_DO_NOT_BUILD':'1',
                                         'ELASTOS_BROWSER_LINUX_UAPI_INCLUDE':str(headers)}), \
                 patch.object(builder.subprocess, 'run', side_effect=run), \
                 patch.object(upstream, 'builder_module', return_value=builder):
                receipt = upstream.package(recipe, root / 'cache', root / 'output')
            platform_input.check_upstream_archive(root / 'output' / receipt['release_path'], recipe, receipt, 'aarch64-linux')
            self.assertTrue(any('libcap.a' in call and 'SHARED=no' in call for call in calls))
            with tarfile.open(root / 'output' / receipt['release_path']) as archive:
                vendor = archive.extractfile(recipe['root'] + '/sources/cargo-vendor.tar.gz')
                with tarfile.open(fileobj=vendor, mode='r:gz') as locked:
                    self.assertEqual(locked.extractfile('registry-vendor/registry-crate.txt').read(), b'locked registry fixture')
                    self.assertTrue(all(m.uid == m.gid == 0 and m.uname == m.gname == '' for m in locked.getmembers()))
                with tarfile.open(fileobj=archive.extractfile(recipe['root'] + '/sources/linux-uapi.tar.gz'), mode='r:gz') as uapi:
                    self.assertEqual(uapi.extractfile('linux-uapi/linux/fixture.h').read(), b'build header fixture')


if __name__ == '__main__':
    unittest.main()
