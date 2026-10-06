#!/usr/bin/env python3
"""Capacity regressions use injected tools and disks; they remove no host tools."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('controller', Path(__file__).with_name('release-mac-capacity.py'))
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)
HELP = "delete: use alias 'all'; list"


class CapacityTests(unittest.TestCase):
    def setUp(self):
        self.record, self.growth, self.clock, self.commands = {}, 100, 0, []
        self.disk = SimpleNamespace(total=1000, free=100)
        self.bindings = Mock(return_value={'sdk': 'native', 'clang_sha256': 'a'})
        self.images = [{'image': {'state': 'Ready'}}, {'image': {'state': 'Deleting'}}, {}]

    def query(self, argv, timeout=30):
        self.commands.append((argv, timeout))
        if argv[-2:] == ['help', 'runtime']:
            return HELP
        if argv[-3:] == ['runtime', 'list', '-j']:
            value = self.images.pop(0)
            if not value:
                self.disk.free = 400
            return json.dumps(value)
        return ''

    def refusal(self, *, status='unavailable', growth=100, kind=ValueError):
        error = kind(f'hosted Mac capacity unavailable: free=100 total=1000 planned_growth={growth}')
        error.capacity = {'status': status, 'planned_growth_bytes': growth, 'free_bytes_after': 100, 'total_bytes': 1000}
        return error

    def run_capacity(self, prepare, **overrides):
        args = dict(bindings=self.bindings, query=self.query, monotonic=lambda: self.clock,
                    sleep=lambda seconds: setattr(self, 'clock', self.clock + seconds))
        args.update(overrides)
        controller.run_capacity(self.record, self.growth, prepare, lambda: self.disk, **args)

    def test_query_reads_stderr_only_for_runtime_help(self):
        help_command = ['/usr/bin/xcrun', 'simctl', 'help', 'runtime']
        list_command = ['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']
        replies = [subprocess.CompletedProcess(help_command, 0, stdout='', stderr=HELP),
                   subprocess.CompletedProcess(list_command, 0, stdout='{}\n', stderr='simctl warning\n')]
        with patch.object(controller.subprocess, 'run', side_effect=replies):
            self.assertEqual(controller.query(help_command, timeout=15), HELP)
            self.assertEqual(json.loads(controller.query(list_command)), {})

    def test_enough_capacity_skips_simulator_removal(self):
        self.disk.free = 250
        prepare = Mock(return_value={'status': 'ready'})
        self.run_capacity(prepare)
        self.assertEqual(self.record['status'], 'ready')
        self.assertEqual(self.commands, [])
        prepare.assert_called_once()

    def test_shortfall_removes_simulators_waits_then_rechecks(self):
        error = self.refusal()
        prepare = Mock(side_effect=[error, {'status': 'ready'}])
        self.run_capacity(prepare)
        self.assertIs(self.record['reclaim'], error.capacity)
        self.assertEqual((self.record['status'], self.clock, prepare.call_count), ('ready', 1, 2))
        self.assertEqual([command[2:] for command, _ in self.commands], [
            ['help', 'runtime'], ['runtime', 'list', '-j'], ['shutdown', 'all'], ['delete', 'all'],
            ['runtime', 'delete', 'all'], ['runtime', 'list', '-j'], ['runtime', 'list', '-j']])

    def test_other_errors_rethrow_without_touching_simulators(self):
        for error in (ValueError('other failure'), self.refusal(status='ready'), self.refusal(growth=99),
                      self.refusal(kind=OSError)):
            with self.subTest(error=error), self.assertRaises(type(error)) as caught:
                self.run_capacity(Mock(side_effect=error))
            self.assertIs(caught.exception, error)
            self.assertEqual(self.commands, [])

    def test_shortfall_claim_cannot_override_measured_space_or_changed_tools(self):
        self.disk.free = 250
        with self.assertRaises(ValueError):
            self.run_capacity(Mock(side_effect=self.refusal()))
        self.disk.free = 100
        self.bindings.side_effect = [{'sdk': 'native'}, {'sdk': 'changed'}]
        with self.assertRaisesRegex(ValueError, 'Native build inputs changed'):
            self.run_capacity(Mock(side_effect=self.refusal()))
        self.assertEqual(self.commands, [])

    def test_unsupported_simctl_help_refuses_before_removal(self):
        calls = []
        def query(argv, timeout=30):
            calls.append(argv)
            return 'unsupported help' if argv[-2:] == ['help', 'runtime'] else '{}'
        with self.assertRaisesRegex(ValueError, 'unavailable'):
            self.run_capacity(Mock(side_effect=self.refusal()), query=query)
        self.assertFalse(any(command[-1] == 'all' for command in calls))

    def test_runtime_removal_has_a_deadline(self):
        def pending(argv, timeout=30):
            return HELP if argv[-2:] == ['help', 'runtime'] else json.dumps({'image': {'state': 'Deleting'}})
        prepare = Mock(side_effect=self.refusal())
        with self.assertRaisesRegex(ValueError, 'timed out'):
            self.run_capacity(prepare, query=pending)
        self.assertEqual(self.clock, 900)
        prepare.assert_called_once()

    def test_wait_ends_once_space_is_met(self):
        def pending(argv, timeout=30):
            if argv[-3:] == ['runtime', 'list', '-j'] and self.clock >= 2:
                self.disk.free = 250
            return HELP if argv[-2:] == ['help', 'runtime'] else json.dumps({'image': {}})
        self.run_capacity(Mock(side_effect=[self.refusal(), {'status': 'ready'}]), query=pending)
        self.assertEqual((self.record['status'], self.clock), ('ready', 2))

    def test_changed_tools_after_simulators_refuse_second_pass(self):
        self.bindings.side_effect = [{'sdk': 'native'}, {'sdk': 'native'}, {'sdk': 'changed'}]
        prepare = Mock(side_effect=self.refusal())
        with self.assertRaisesRegex(ValueError, 'Native build inputs changed'):
            self.run_capacity(prepare)
        prepare.assert_called_once()

    def test_second_shortfall_stays_failed(self):
        second = self.refusal()
        with self.assertRaises(ValueError) as caught:
            self.run_capacity(Mock(side_effect=[self.refusal(), second]))
        self.assertIs(caught.exception, second)
        self.assertNotEqual(self.record.get('status'), 'ready')

    def test_reserve_boundary_is_exact(self):
        self.assertTrue(controller.enough(SimpleNamespace(total=1000, free=250), 100))
        self.assertFalse(controller.enough(SimpleNamespace(total=1000, free=249), 100))

    def test_entrypoint_refuses_local_and_self_hosted_runs(self):
        with patch.object(sys, 'argv', ['capacity']), patch.object(controller, 'query') as query:
            for environment in ({'CI': 'true', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'self-hosted'},
                                {'CI': 'false', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted'}):
                with patch.dict(os.environ, environment), self.assertRaisesRegex(ValueError, 'disposable hosted Mac'):
                    controller.main()
            query.assert_not_called()

    def test_failure_still_writes_a_protected_capacity_record(self):
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory).resolve()
            root = temp / 'release-123-1'
            root.mkdir(mode=0o700)
            fake = SimpleNamespace(cli_reclaim_xcode=lambda *args: None)
            original = fake.cli_reclaim_xcode
            fake.cli_prepare_ci_disk = Mock(side_effect=self.refusal(growth=50 * 1024**3))
            environment = {'CI': 'true', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
                           'RUNNER_TEMP': str(temp), 'RELEASE_ROOT': str(root), 'GITHUB_RUN_ID': '123',
                           'GITHUB_RUN_ATTEMPT': '1', 'SOURCE_COMMIT': 'a' * 40, 'SOURCE_TREE': 'b' * 40}
            def run(record, growth, prepare, measure):
                prepare()
            with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['capacity']), \
                 patch.object(sys, 'platform', 'darwin'), \
                 patch.object(controller.pwd, 'getpwuid', return_value=SimpleNamespace(pw_name='runner')), \
                 patch.object(Path, 'is_relative_to', return_value=True), \
                 patch.object(controller, 'query', side_effect=['a' * 40, 'b' * 40, '']), \
                 patch.object(controller.importlib.util, 'spec_from_file_location',
                              return_value=SimpleNamespace(loader=SimpleNamespace(exec_module=lambda module: None))), \
                 patch.object(controller.importlib.util, 'module_from_spec', return_value=fake), \
                 patch.object(controller, 'run_capacity', side_effect=run), \
                 patch.object(controller.shutil, 'disk_usage', return_value=SimpleNamespace(total=1000, free=100)), \
                 self.assertRaises(ValueError):
                controller.main()
            self.assertIs(fake.cli_reclaim_xcode, original)
            record = json.loads((root / 'capacity.json').read_text())
            self.assertEqual((root / 'capacity.json').stat().st_mode & 0o777, 0o600)
            self.assertEqual((record['status'], record['source_commit'], record['free_bytes']), ('unavailable', 'a' * 40, 100))

    def test_native_bindings_hash_tools_and_sdk_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            developer = Path(directory).resolve() / 'Xcode.app/Contents/Developer'
            sdk = developer / 'SDKs/MacOSX.sdk'
            sdk.mkdir(parents=True)
            paths = {name: developer / name for name in ('clang', 'ld', 'simctl')}
            paths['sdk_settings'] = sdk / 'SDKSettings.json'
            for name, path in paths.items():
                path.write_bytes(name.encode())
            def query(argv, timeout=30):
                if argv[0] == '/usr/bin/xcode-select':
                    return str(developer)
                return str(sdk) if argv[-1] == '--show-sdk-path' else str(paths[argv[-1]])
            with patch.object(controller, 'query', query), patch.object(Path, 'is_relative_to', return_value=True), \
                 patch.dict(os.environ, {'DEVELOPER_DIR': ''}):
                record = controller.native_bindings()
                for name, path in paths.items():
                    self.assertEqual(record[name + '_sha256'], hashlib.sha256(name.encode()).hexdigest())
                paths['clang'].write_bytes(b'changed')
                self.assertNotEqual(controller.native_bindings(), record)
            with patch.object(controller, 'query', lambda argv: 'relative'), self.assertRaisesRegex(ValueError, 'must be absolute'):
                controller.native_bindings()


if __name__ == '__main__':
    unittest.main()
