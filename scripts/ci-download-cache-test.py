#!/usr/bin/env python3
"""Exercise CI input caches and bounded failures with inert local fixtures."""
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import ssl
import sys
import tempfile
import unittest
from unittest import mock
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('kubo', ROOT / 'scripts/ci-kubo-cache.py')
kubo = importlib.util.module_from_spec(spec)
spec.loader.exec_module(kubo)


class KuboCacheTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.cache = self.root / 'cache'
        self.archive = self.root / 'archive'
        self.archive.write_bytes(b'inert pinned archive')
        self.source = {'path': str(self.archive), 'max_bytes': 512 * 1024**2,
                       'checksum': 'sha512:' + hashlib.sha512(self.archive.read_bytes()).hexdigest()}

    def test_cache_hit_rehashes_pin_and_avoids_network(self):
        path = kubo.fetch(self.source, self.cache)
        self.archive.unlink()
        with mock.patch.object(kubo.subprocess, 'run') as child:
            self.assertEqual(kubo.fetch(self.source, self.cache), path)
            child.assert_not_called()
            path.write_bytes(b'corrupt restore')
            with self.assertRaisesRegex(ValueError, 'cached upstream'):
                kubo.fetch(self.source, self.cache)
            child.assert_not_called()

    def test_transport_failure_retries_then_publishes_verified_bytes(self):
        run = subprocess.run
        with mock.patch.object(kubo, 'sleep') as sleep, \
             mock.patch.object(kubo.subprocess, 'run', side_effect=None) as child:
            # Use a callable second result so the real subprocess verifies the fixture.
            child.side_effect = lambda *a, **kw: (subprocess.CompletedProcess([], 75)
                if child.call_count == 1 else run(*a, **kw))
            path = kubo.fetch(self.source, self.cache)
            self.assertEqual(path.read_bytes(), self.archive.read_bytes())
            self.assertEqual(child.call_count, 2)

    def test_timeout_exhaustion_removes_partial_files_and_uses_three_attempts(self):
        def stall(command, **kwargs):
            (Path(command[-1]) / 'partial').write_bytes(b'unverified partial')
            raise subprocess.TimeoutExpired(command, kwargs['timeout'])
        with mock.patch.object(kubo, 'sleep') as sleep, \
             mock.patch.object(kubo.subprocess, 'run', side_effect=stall) as child:
            with self.assertRaisesRegex(ValueError, 'three bounded attempts'):
                kubo.fetch(self.source, self.cache)
            self.assertEqual(child.call_count, 3)
            self.assertEqual(sleep.call_count, 2)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_checksum_failure_has_one_attempt_and_no_cached_file(self):
        self.archive.write_bytes(b'corrupt transport response')
        run = subprocess.run
        with mock.patch.object(kubo.subprocess, 'run', wraps=run) as child:
            with self.assertRaisesRegex(ValueError, 'verification failed'):
                kubo.fetch(self.source, self.cache)
            self.assertEqual(child.call_count, 1)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_transport_errors_are_retryable_and_integrity_errors_are_terminal(self):
        for error, expected in ((urllib.error.URLError('mirror stalled'), 75),
                                (http.client.IncompleteRead(b'partial'), 75),
                                (ssl.SSLError('bad record MAC'), 75),
                                (OSError('body read failed'), 75),
                                (ValueError('wrong checksum'), 1)):
            with mock.patch.object(sys, 'argv', ['ci-kubo-cache.py', '--fetch-one', str(self.cache)]), \
                 mock.patch.object(kubo.json, 'load', return_value=self.source), \
                 mock.patch.object(kubo.upstream, 'cached_input', side_effect=error):
                self.assertEqual(kubo.main(), expected)


class AptRetryTests(unittest.TestCase):
    def run_install(self, corrupt=False, bad_download=False):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory).resolve()
            archives, etc, shims = root / 'archives', root / 'etc', root / 'shims'
            for path in (archives, etc, shims):
                path.mkdir()
            (etc / 'sources.list').write_text('deb https://archive.ubuntu.com/ubuntu noble main\n')
            payload = b'inert Ubuntu package'
            (archives / 'fixture_1_all.deb').write_bytes(b'x' * len(payload) if corrupt else payload)
            commands = {
                'sudo': 'os.execvp(args[0], args)',
                'chown': 'pass',
                'install': 'pass',
                'dpkg-deb': "print('fixture\\n1\\nall')",
                'apt-cache': "print('SHA256: ' + os.environ['FIXTURE_SHA256'])",
                'timeout': "os.execvp(args[2], args[2:])",
                'apt-get': '''
if '--download-only' in args:
    pathlib.Path(os.environ['FIXTURE_ARCHIVES'], 'fixture_1_all.deb').write_bytes(
        b'bad download' if os.environ['FIXTURE_BAD_DOWNLOAD'] == '1' else b'inert Ubuntu package')
'''
            }
            for command, body in commands.items():
                shim = shims / command
                shim.write_text('#!' + sys.executable + '\nimport json, os, pathlib, sys\n'
                    'args = sys.argv[1:]\nlog = pathlib.Path(os.environ["FIXTURE_LOG"])\n'
                    'with log.open("a") as stream:\n'
                    f'    stream.write(json.dumps({{"command": {command!r}, "args": args}}) + "\\n")\n' + body)
                shim.chmod(0o700)
            script = (ROOT / 'scripts/ci-apt-prerequisites.sh').read_text()
            script = script.replace('/var/cache/apt/archives', str(archives)).replace('/etc/apt', str(etc))
            result = subprocess.run(['bash', '-c', script], text=True, capture_output=True, timeout=15,
                env={**os.environ, 'PATH': str(shims) + os.pathsep + os.environ['PATH'],
                     'CI_APT_PACKAGES': 'coturn e2fsprogs ffmpeg musl-tools nasm pkg-config',
                     'FIXTURE_LOG': str(root / 'log'), 'FIXTURE_SHA256': hashlib.sha256(payload).hexdigest(),
                     'FIXTURE_BAD_DOWNLOAD': '1' if bad_download else '0',
                     'FIXTURE_ARCHIVES': str(archives), 'GITHUB_OUTPUT': str(root / 'output')})
            events = [json.loads(line) for line in (root / 'log').read_text().splitlines()]
            output = (root / 'output').read_text() if (root / 'output').exists() else ''
            return result, events, output

    def test_corrupt_restore_is_removed_before_download(self):
        result, _, output = self.run_install(corrupt=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Removed unverified apt archive', result.stderr)
        self.assertEqual(output, 'changed=true\n')

    def test_corrupt_download_stops_before_package_install(self):
        result, events, output = self.run_install(bad_download=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('signed-index SHA-256 check', result.stderr)
        self.assertFalse(any('--no-download' in e['args'] for e in events if e['command'] == 'apt-get'))
        self.assertEqual(output, '')

    def test_unchanged_verified_archive_set_skips_save(self):
        result, _, output = self.run_install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(output, 'changed=false\n')


if __name__ == '__main__':
    unittest.main()
