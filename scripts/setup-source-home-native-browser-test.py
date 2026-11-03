#!/usr/bin/env python3
"""Run the complete source-home installer with inert external build/Runtime tools."""
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]
SETUP = ROOT / "scripts/setup-source-home.sh"
PLATFORM = "linux-arm64"
APPS = ("home-cli home-gui home system services people browser documents library "
        "elacity-player marketplace archive-manager inbox assistant wallet wallet-metamask "
        "wallet-unisat wallet-walletconnect gba-emulator gba-ucity gba-nonogram chat-room").split()
SUPPORT = (
    "setup-source-home.sh", "browser-source-home-config.mjs", "browser-runtime-turn.mjs",
    "browser-selkies-control-service.mjs", "browser-vm-engine-supervisor.mjs",
    "browser-vm-control-service.mjs", "browser-vm-remote-vz-launcher.mjs",
    "browser-vm-vz-transport-bootstrap.mjs", "browser-vm-linux-network.py",
    "browser-vm-engine-preflight.sh", "browser-vm-artifact-preflight.sh",
    "browser-vm-target-preflight.sh", "setup-source-home-browser-artifacts.sh",
    "stamp-source-home-capsule-metadata.py", "components-release-integrity-check.py",
    "install-source-home-runtime.py", "release-platforms.json", "lib/owned_removal.py",
)
CARGO = r'''import json, os, pathlib, sys
args = sys.argv[1:]
assert args[0] == 'build' and '--locked' in args, args
manifest = pathlib.Path(args[args.index('--manifest-path') + 1])
name = args[args.index('--bin') + 1] if '--bin' in args else manifest.parent.name
out = pathlib.Path(os.environ['CARGO_TARGET_DIR']) / 'release' / name
out.parent.mkdir(parents=True, exist_ok=True)
out.write_text('inert native artifact: ' + name)
out.chmod(0o700)
with open(os.environ['FIXTURE_EVENTS'], 'a') as log:
    log.write(json.dumps({'phase': 'build', 'name': name, 'args': args}) + '\n')
'''
RUNTIME = r'''import hashlib, json, os, pathlib, sys
args = sys.argv[1:]
data = pathlib.Path(os.environ['XDG_DATA_HOME']) / 'elastos'
event = {'phase': 'runtime', 'args': args}
if args[:3] == ['setup', '--with', 'media-provider']:
    assert '--prerequisites-only' in args
    assert '--media-tools-dir' in args
elif args == ['setup', '--profile', 'source-home', '--without', 'media-provider']:
    p = pathlib.Path(os.environ['ELASTOS_COMPONENTS_MANIFEST'])
    assert p.read_bytes() == (data / 'components.json').read_bytes()
    assert p.with_name('model-catalog.json').read_bytes() == pathlib.Path(os.environ['FIXTURE_CATALOG']).read_bytes()
    assert p.parent.stat().st_mode & 0o777 == 0o700
    assert p.stat().st_mode & 0o777 == 0o600
    assert p.with_name('model-catalog.json').stat().st_mode & 0o777 == 0o600
    manifest = json.loads(p.read_text())
    native = data / 'bin/browser-vz-engine-supervisor'
    expected = 'sha256:' + hashlib.sha256(native.read_bytes()).hexdigest()
    assert manifest['external']['browser-vz-engine-supervisor']['platforms']['linux-arm64']['checksum'] == expected
    assert manifest['capsules']['browser']['entrypoint_sha256'].startswith('sha256:')
    assert (data / 'receipts/source-home-capsules.json').is_file()
    event.update(stage=str(p.parent), native_sha=expected,
                 components_sha='sha256:' + hashlib.sha256(p.read_bytes()).hexdigest(),
                 runtime_installed=(data / 'bin/elastos').exists())
    replacement = os.environ.get('STAGE_REPLACEMENT', '')
    if replacement:
        stage = p.parent
        stage.rename(stage.with_name(stage.name + '.original'))
        if replacement == 'directory':
            stage.mkdir()
        else:
            outside = pathlib.Path(os.environ['FIXTURE_OUTSIDE']) / stage.name
            outside.mkdir(parents=True)
            stage.symlink_to(outside, target_is_directory=True)
        (stage / 'replacement-owner').write_text('keep')
else:
    raise AssertionError(args)
with open(os.environ['FIXTURE_EVENTS'], 'a') as log:
    log.write(json.dumps(event) + '\n')
if '--profile' in args:
    sys.exit(int(os.environ.get('SETUP_RESULT', '0')))
'''


def write(path, content, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content)
    path.chmod(mode)


class NativeSourceHomeTest(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix='native-source-home-')
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.repo = self.root / 'source'
        self.repo.mkdir()
        self.data = self.root / 'xdg/elastos'
        self.data.mkdir(parents=True)
        self.commands = self.root / 'commands'
        self.commands.mkdir()
        self.events = self.root / 'events.jsonl'
        self.runtime = self.root / 'runtime'
        for name in SUPPORT:
            target = self.repo / 'scripts' / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / 'scripts' / name, target)
            target.chmod(0o755)
        self.assertEqual((self.repo / 'scripts/setup-source-home.sh').read_bytes(), SETUP.read_bytes())
        external = {}
        for name in APPS:
            capsule = self.repo / 'capsules' / name
            write(capsule / 'browser/index.html', 'fixture ' + name)
            write(capsule / 'capsule.json', json.dumps({
                'name': name, 'type': 'web-projection', 'runtime_abi': 'elastos.runtime-projection/v1',
                'execution': 'web-projection', 'entrypoint': 'browser/index.html'}))
            external[name] = {'install_path': 'capsules/' + name,
                              'platforms': {PLATFORM: {'strategy': 'source-build'}}}
        for manifest in ('elastos/Cargo.toml', 'elastos/capsules/shell/Cargo.toml',
                         'capsules/browser-local-exit/Cargo.toml', 'capsules/home-cli/Cargo.toml'):
            write(self.repo / manifest, '[package]\n')
        for name in ('shell', 'browser-local-exit'):
            external[name] = {'install_path': 'bin/' + name,
                              'platforms': {PLATFORM: {'strategy': 'source-build'}}}
        for name in ('operator-drive-adapter', 'drm-provider', 'rights-provider', 'key-provider', 'decrypt-provider'):
            external[name] = {'platforms': {}}
        repository_manifest = json.loads((ROOT / 'components.json').read_text())
        host_profile = repository_manifest['profiles']['browser-host']
        for name in host_profile['components']:
            external[name] = repository_manifest['external'][name]
        self.manifest = {'external': external, 'profiles': {'browser-host': host_profile}}
        self.save_manifest()
        write(self.repo / 'model-catalog.json', '{"catalog":"fixture"}\n')
        write(self.runtime, '#!' + sys.executable + '\n' + RUNTIME, 0o700)
        write(self.commands / 'cargo', '#!' + sys.executable + '\n' + CARGO, 0o700)
        write(self.commands / 'rustc', '#!/bin/sh\necho rustc 1.91.0\n', 0o700)
        write(self.commands / 'uname', '#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo "${FIXTURE_ARCH:-aarch64}";; *) exit 99;; esac\n', 0o700)
        write(self.commands / 'df', '#!/bin/sh\nprintf "Filesystem 1024-blocks Used Available Capacity Mounted\nfixture 100000000 1 90000000 1%% /\n"\n', 0o700)
        write(self.commands / 'git', '#!/bin/sh\ncase "$3" in status) exit 0;; rev-parse) printf "%040d\\n" 1;; *) exit 99;; esac\n', 0o700)
        media = self.root / 'media'
        media.mkdir(mode=0o700)
        for name in ('ffmpeg', 'ffprobe'):
            write(media / name, '#!/bin/sh\nexit 99\n', 0o700)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(('ELASTOS_', 'SETUP_SOURCE_HOME_', 'XDG_', 'CARGO_', 'RUSTUP_'))}
        self.env.update(HOME=str(self.root / 'home'), XDG_DATA_HOME=str(self.data.parent),
                        PATH=str(self.commands) + os.pathsep + os.environ['PATH'],
                        ELASTOS_NODE_BIN=shutil.which('node'), ELASTOS_CARGO_BIN=str(self.commands / 'cargo'),
                        CARGO_TARGET_DIR=str(self.root / 'built'), ELASTOS_COLLABORATION_STARTUP_MODE='isolated',
                        SETUP_SOURCE_HOME_RUNTIME_BIN=str(self.runtime), SETUP_SOURCE_HOME_INSTALL_KUBO='0',
                        SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR=str(media), SETUP_SOURCE_HOME_INSTALL_LLAMA_SERVER='0',
                        FIXTURE_EVENTS=str(self.events), FIXTURE_CATALOG=str(self.repo / 'model-catalog.json'),
                        FIXTURE_OUTSIDE=str(self.root / 'outside'), PYTHONDONTWRITEBYTECODE='1')

    def save_manifest(self):
        write(self.repo / 'components.json', json.dumps(self.manifest))

    def run_setup(self, overrides=None):
        return subprocess.run(['bash', str(self.repo / 'scripts/setup-source-home.sh')],
                              env={**self.env, **(overrides or {})},
                              capture_output=True, text=True, timeout=60)

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stderr + result.stdout)

    def recorded_events(self):
        return [json.loads(line) for line in self.events.read_text().splitlines()]

    def test_config_only_uses_explicit_platform_and_updates_idempotently(self):
        for platform, machine in [('linux-arm64', 'aarch64'), ('linux-amd64', 'x86_64')]:
            with self.subTest(platform=platform):
                xdg = self.root / platform
                for _ in range(2):
                    self.assert_ok(self.run_setup({'SETUP_SOURCE_HOME_CONFIG_ONLY': '1',
                                                  'FIXTURE_ARCH': machine, 'XDG_DATA_HOME': str(xdg)}))
                    data = xdg / 'elastos'
                    config = json.loads((data / 'config/browser-engine-adapter.json').read_text())
                    if platform == 'linux-amd64':
                        self.assertEqual(config['adapters'], [])
                        continue
                    owner = config['adapters'][0]['supervisor']['env']
                    self.assertEqual(owner['ELASTOS_BROWSER_VM_CONTROL_LAUNCHER'], str(data / 'bin/browser-vz-engine-supervisor'))
                    # The native owner takes a prepared rootfs copy per launch (#90).
                    self.assertEqual(owner['ELASTOS_BROWSER_VM_ROOTFS_POOL_DIR'], str(data / 'browser-vm/rootfs-pool'))
                    self.assertNotIn('ELASTOS_BROWSER_VM_ROOTFS_COPY_MODE', owner)
                    self.assertNotIn('ELASTOS_BROWSER_VM_ICE_SERVERS_JSON', owner)
                    transport = data / 'config/browser-vz-vsock-transport.json'
                    self.assertEqual(transport.stat().st_mode & 0o777, 0o600)
                    self.assertTrue(json.loads(transport.read_text())['enabled'])
                    self.assertFalse((data / 'runtime-turn/turn-credentials.env').exists())
        self.assertFalse(self.events.exists(), 'config-only must not reach a build or Runtime setup')

    def test_full_setup_installs_native_owner_and_publishes_exact_receipt(self):
        self.assert_ok(self.run_setup())
        installed = self.data / 'bin/browser-vz-engine-supervisor'
        self.assertEqual(installed.read_bytes(), (self.root / 'built/release/browser-vz-engine-supervisor').read_bytes())
        self.assertEqual((self.data / 'bin/elastos').read_bytes(), self.runtime.read_bytes())
        receipt = json.loads((self.data / 'receipts/source-home-installation.json').read_text())
        self.assertEqual(receipt['runtime']['installed_sha256'], 'sha256:' + hashlib.sha256(self.runtime.read_bytes()).hexdigest())
        # Runtime setup admits only prepared Browser host rows (browser_config::host_rows).
        stamped = json.loads((self.data / 'components.json').read_text())
        for name in stamped['profiles']['browser-host']['components']:
            info = stamped['external'][name]['platforms'].get(PLATFORM)
            if info is not None and name in stamped['profiles']['source-home']['components']:
                self.assertEqual(info.get('strategy'), 'prebuilt', name)
        events = self.recorded_events()
        self.assertEqual([event['name'] for event in events if event['phase'] == 'build'],
                         ['browser-vz-engine-supervisor', 'shell', 'browser-local-exit', 'home-cli'])
        self.assertEqual([event['args'][:3] for event in events if event['phase'] == 'runtime'],
                         [['setup', '--with', 'media-provider'], ['setup', '--profile', 'source-home']])
        self.assertEqual(receipt['components_sha256'], events[-1]['components_sha'])
        self.assertFalse(events[-1]['runtime_installed'], 'canonical setup precedes Runtime publication')
        self.assertFalse(Path(events[-1]['stage']).exists())
        self.assertFalse((self.data / 'bin/browser-vm-local-crosvm-launcher').exists())
        self.assertFalse((self.data / 'scripts/browser-vm-prepare-rootfs-pool.mjs').exists())

    def test_canonical_setup_status_and_identity_bound_stage_cleanup(self):
        for replacement in ('', 'directory', 'symlink'):
            for status in (0, 23):
                with self.subTest(replacement=replacement, status=status):
                    result = self.run_setup({'STAGE_REPLACEMENT': replacement, 'SETUP_RESULT': str(status)})
                    self.assertEqual(result.returncode, status or (1 if replacement else 0), result.stderr + result.stdout)
                    event = self.recorded_events()[-1]
                    stage = Path(event['stage'])
                    if replacement:
                        self.assertIn('stage cleanup failed', result.stderr)
                        self.assertEqual((stage / 'replacement-owner').read_text(), 'keep')
                        self.assertEqual(stage.is_symlink(), replacement == 'symlink')
                        self.assertTrue((stage.with_name(stage.name + '.original') / 'components.json').is_file())
                    else:
                        self.assertFalse(stage.exists())

    def test_whole_setup_retains_and_rechecks_node_and_transport_pins(self):
        previous = {'external': {}}
        for bundle in ('node', 'vhost-device-vsock', 'crosvm'):
            info = {'strategy': 'source-build', 'install_path': f'libexec/browser/{bundle}/pin',
                    'release_path': f'{bundle}-{PLATFORM}.tar.gz', 'extract_path': bundle + '-pin', 'binary_path': 'bin/' + bundle}
            self.manifest['external'][bundle] = {'version': 'pin', 'platforms': {PLATFORM: info}}
            checksum = 'sha256:' + hashlib.sha256(bundle.encode()).hexdigest()
            write(self.data / info['install_path'] / '.elastos-artifact-sha256', checksum + '\n')
            previous['external'][bundle] = {'version': 'pin', 'platforms': {PLATFORM: {
                **info, 'strategy': 'prebuilt', 'checksum': checksum, 'size': 123}}}
        self.save_manifest()
        write(self.data / 'components.json', json.dumps(previous))
        for _ in range(2):
            self.assert_ok(self.run_setup())
            manifest = json.loads((self.data / 'components.json').read_text())
            for bundle in previous['external']:
                self.assertEqual(manifest['external'][bundle]['platforms'][PLATFORM], previous['external'][bundle]['platforms'][PLATFORM])
                self.assertIn(bundle, manifest['profiles']['source-home']['components'])
        for bundle in previous['external']:
            with self.subTest(bundle=bundle):
                marker = self.data / previous['external'][bundle]['platforms'][PLATFORM]['install_path'] / '.elastos-artifact-sha256'
                saved = marker.read_text()
                marker.write_text('b' * 64)
                result = self.run_setup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('archive receipt differs', result.stderr)
                self.assertNotIn('stage', self.recorded_events()[-1])
                marker.write_text(saved)
                self.manifest['external'][bundle]['version'] = 'changed'
                self.save_manifest()
                result = self.run_setup()
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('different source inputs', result.stderr)
                self.manifest['external'][bundle]['version'] = 'pin'
                self.save_manifest()

    def test_legacy_turn_retirement_preserves_records_under_linked_parents(self):
        outside = self.root / 'outside-turn'
        outside.mkdir()
        record = outside / 'turnserver.pid'
        record.write_text('99999999')
        (self.data / 'runtime-turn').symlink_to(outside, target_is_directory=True)
        failure = self.run_setup()
        self.assertNotEqual(failure.returncode, 0)
        self.assertIn('requires repair', failure.stderr)
        self.assertEqual(record.read_text(), '99999999')
        (self.data / 'runtime-turn').unlink()
        (self.data / 'runtime-turn').mkdir()
        home_record = self.data / 'runtime-turn/turnserver.pid'
        home_record.write_text('99999999')
        linked = self.root / 'linked-parent'
        linked.symlink_to(self.data.parent, target_is_directory=True)
        failure = self.run_setup({'XDG_DATA_HOME': str(linked)})
        self.assertNotEqual(failure.returncode, 0)
        self.assertIn('requires repair', failure.stderr)
        self.assertEqual(home_record.read_text(), '99999999')

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open'), 'Linux pidfd ownership')
    def test_legacy_turn_retirement_keeps_pid_record_replaced_during_shutdown(self):
        directory = self.data / 'runtime-turn'
        directory.mkdir()
        (self.data / 'bin').mkdir()
        (self.data / 'bin/turnserver').symlink_to(sys.executable)
        config = directory / 'turnserver.conf'
        record = directory / 'turnserver.pid'
        program = textwrap.dedent('''\
            import os, signal, sys
            from pathlib import Path
            record = Path(sys.argv[-1]).with_name('turnserver.pid')
            def replace_record(*_):
                replacement = record.with_suffix('.new')
                replacement.write_text('replacement-owner')
                os.replace(replacement, record)
                sys.exit(0)
            signal.signal(signal.SIGTERM, replace_record)
            print('ready', flush=True)
            signal.pause()
        ''')
        child = subprocess.Popen([sys.executable, '-c', program, '-c', str(config)], stdout=subprocess.PIPE, text=True)
        try:
            self.assertEqual(child.stdout.readline().strip(), 'ready')
            record.write_text(str(child.pid))
            failure = self.run_setup()
            child.wait(timeout=5)
            self.assertNotEqual(failure.returncode, 0)
            self.assertIn('requires repair', failure.stderr)
            self.assertEqual(record.read_text(), 'replacement-owner')
        finally:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=5)
            child.stdout.close()

    @unittest.skipUnless(sys.platform == 'linux' and hasattr(os, 'pidfd_open'), 'Linux pidfd ownership')
    def test_legacy_turn_retirement_stops_exact_owner_and_preserves_unrelated_pid(self):
        directory = self.data / 'runtime-turn'
        directory.mkdir()
        (self.data / 'bin').mkdir()
        (self.data / 'bin/turnserver').symlink_to(sys.executable)
        config = directory / 'turnserver.conf'
        pid_file = directory / 'turnserver.pid'
        program = 'import signal; signal.pause()'
        owned = subprocess.Popen([sys.executable, '-c', program, '-c', str(config)])
        unrelated = subprocess.Popen([sys.executable, '-c', program])
        try:
            pid_file.write_text(str(owned.pid))
            self.assert_ok(self.run_setup())
            owned.wait(timeout=5)
            self.assertEqual(owned.returncode, -15)
            self.assertFalse(pid_file.exists())
            pid_file.write_text(str(unrelated.pid))
            self.assert_ok(self.run_setup())
            self.assertIsNone(unrelated.poll())
            self.assertFalse(pid_file.exists())
            # An explicit old executable with the same Home config needs
            # repair; retain its owner record and process for that repair.
            (self.data / 'bin/turnserver').unlink()
            (self.data / 'bin/turnserver').symlink_to('/bin/true')
            explicit = subprocess.Popen([sys.executable, '-c', program, '-c', str(config)])
            try:
                pid_file.write_text(str(explicit.pid))
                failure = self.run_setup()
                self.assertNotEqual(failure.returncode, 0)
                self.assertIn('executable requires repair', failure.stderr)
                self.assertEqual(pid_file.read_text(), str(explicit.pid))
                self.assertIsNone(explicit.poll())
            finally:
                explicit.terminate()
                explicit.wait(timeout=5)
        finally:
            for child in (owned, unrelated):
                if child.poll() is None:
                    child.terminate()
                child.wait(timeout=5)

if __name__ == "__main__":
    unittest.main()
