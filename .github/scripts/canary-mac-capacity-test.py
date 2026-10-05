#!/usr/bin/env python3
"""Capacity regressions use injected tools and disks; they remove no host tools."""
import importlib.util
import hashlib
import json
import os
from pathlib import Path
import sys
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import Mock, patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location('controller', Path(__file__).with_name('canary-mac-capacity.py'))
controller = importlib.util.module_from_spec(spec)
spec.loader.exec_module(controller)


class CapacityTests(unittest.TestCase):
    def test_query_reads_stderr_only_help_and_preserves_stdout_only_json(self):
        help_command = ['/usr/bin/xcrun', 'simctl', 'help', 'runtime']
        list_command = ['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']
        help_text = "delete: use alias 'all'; list"
        replies = [subprocess.CompletedProcess(help_command, 0, stdout='', stderr=help_text),
                   subprocess.CompletedProcess(list_command, 0, stdout='{}\n', stderr='simctl warning\n')]
        with patch.object(controller.subprocess, 'run', side_effect=replies) as run:
            self.assertEqual(controller.query(help_command, timeout=15), help_text)
            self.assertEqual(json.loads(controller.query(list_command)), {})
        self.assertEqual(run.call_args_list[0].args, (help_command,))
        self.assertEqual(run.call_args_list[0].kwargs,
                         {'capture_output': True, 'text': True, 'check': True, 'timeout': 15})
        self.assertEqual(run.call_args_list[1].args, (list_command,))
        self.assertEqual(run.call_args_list[1].kwargs,
                         {'capture_output': True, 'text': True, 'check': True, 'timeout': 30})

    def setUp(self):
        self.record = {}
        self.growth = 100
        self.disk = SimpleNamespace(total=1000, free=100)
        self.bindings = Mock(return_value={'sdk': 'native', 'clang_sha256': 'a'})
        self.commands = []
        self.images = [{'image': {'state': 'Ready'}}, {'image': {'state': 'Deleting'}}, {}]
        self.clock = 0

        def query(argv, timeout=30):
            self.commands.append((argv, timeout))
            if argv[-2:] == ['help', 'runtime']:
                return "delete: use alias 'all'; list"
            if argv[-3:] == ['runtime', 'list', '-j']:
                value = self.images.pop(0)
                if not value:
                    self.disk.free = 400
                return json.dumps(value)
            return ''
        self.query = query

    def refusal(self, *, status='unavailable', growth=100, kind=ValueError):
        error = kind('hosted Mac capacity unavailable: free=100 total=1000 planned_growth=' + str(growth))
        error.capacity = {'status': status, 'planned_growth_bytes': growth, 'free_bytes_after': 100,
                          'total_bytes': 1000, 'removed': [{'app': 'unused'}]}
        return error

    def run_capacity(self, prepare, **overrides):
        args = dict(bindings=self.bindings, query=self.query, monotonic=lambda: self.clock,
                    sleep=lambda seconds: setattr(self, 'clock', self.clock + seconds))
        args.update(overrides)
        controller.run_capacity(self.record, self.growth, prepare, lambda: self.disk, **args)

    def test_ready_helper_preserves_bindings_and_skips_simulators(self):
        self.disk.free = 250
        prepare = Mock(return_value={'status': 'ready'})
        self.run_capacity(prepare)
        self.assertEqual(self.record['status'], 'ready')
        self.assertEqual(self.commands, [])
        prepare.assert_called_once()

    def test_shortfall_reclaims_fixed_scope_waits_then_rechecks(self):
        error = self.refusal()
        prepare = Mock(side_effect=[error, {'status': 'ready'}])
        self.run_capacity(prepare)
        self.assertIs(self.record['existing_reclaim'], error.capacity)
        self.assertEqual(self.record['status'], 'ready')
        self.assertEqual(self.record['simulator_images_after'], {})
        self.assertEqual(self.clock, 1)
        self.assertEqual(prepare.call_count, 2)
        self.assertEqual([command for command, _ in self.commands], [
            ['/usr/bin/xcrun', 'simctl', 'help', 'runtime'],
            ['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j'],
            ['/usr/bin/xcrun', 'simctl', 'shutdown', 'all'],
            ['/usr/bin/xcrun', 'simctl', 'delete', 'all'],
            ['/usr/bin/xcrun', 'simctl', 'runtime', 'delete', 'all'],
            ['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j'],
            ['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']])
        self.assertEqual([timeout for _, timeout in self.commands], [30, 30, 60, 60, 60, 30, 30])

    def test_arbitrary_or_misclassified_errors_rethrow_without_simulator_reclaim(self):
        ancestry_error = self.refusal()
        ancestry_error.args = ('Android SDK ancestry or owner differs',)
        for error in (ValueError('other failure'), self.refusal(status='ready'), self.refusal(growth=99),
                      self.refusal(kind=OSError), ancestry_error):
            with self.subTest(error=error), self.assertRaises(type(error)) as caught:
                self.run_capacity(Mock(side_effect=error))
            self.assertIs(caught.exception, error)
            self.assertEqual(self.commands, [])

    def test_shortfall_receipt_cannot_override_measured_capacity_or_changed_tools(self):
        self.disk.free = 250
        with self.assertRaises(ValueError):
            self.run_capacity(Mock(side_effect=self.refusal()))
        self.assertEqual(self.commands, [])
        self.disk.free = 100
        self.bindings.side_effect = [{'sdk': 'native'}, {'sdk': 'changed'}]
        with self.assertRaises(ValueError):
            self.run_capacity(Mock(side_effect=self.refusal()))
        self.assertEqual(self.commands, [])

    def test_runtime_inventory_schema_and_help_are_checked_before_mutation(self):
        for reply in ('unsupported help', "delete alias 'all'; list"):
            calls = []
            def query(argv, timeout=30):
                calls.append(argv)
                return reply if argv[-2:] == ['help', 'runtime'] else '[]'
            with self.subTest(reply=reply), self.assertRaises(ValueError):
                self.run_capacity(Mock(side_effect=self.refusal()), query=query)
            self.assertFalse(any(command[-1] == 'all' for command in calls))

    def test_async_runtime_removal_has_a_deadline(self):
        def query(argv, timeout=30):
            return "delete alias 'all'; list" if argv[-2:] == ['help', 'runtime'] else '{}'
        def pending(argv, timeout=30):
            return json.dumps({'image': {'state': 'Deleting'}}) if argv[-3:] == ['runtime', 'list', '-j'] else query(argv, timeout)
        prepare = Mock(side_effect=self.refusal())
        with self.assertRaisesRegex(ValueError, 'timed out'):
            self.run_capacity(prepare, query=pending)
        self.assertEqual(self.clock, 60)
        prepare.assert_called_once()

    def test_simulator_failure_keeps_first_reclaim_receipt(self):
        error = self.refusal()
        def query(argv, timeout=30):
            if argv[-2:] == ['shutdown', 'all']:
                raise OSError('failed simulator command')
            return self.query(argv, timeout)
        with self.assertRaises(OSError):
            self.run_capacity(Mock(side_effect=error), query=query)
        self.assertIs(self.record['existing_reclaim'], error.capacity)

    def test_changed_native_inputs_after_simulators_refuse_second_helper_pass(self):
        self.bindings.side_effect = [{'sdk': 'native'}, {'sdk': 'native'}, {'sdk': 'changed'}]
        prepare = Mock(side_effect=self.refusal())
        with self.assertRaisesRegex(ValueError, 'Native build inputs changed'):
            self.run_capacity(prepare)
        prepare.assert_called_once()

    def test_second_helper_shortfall_stays_failed(self):
        first, second = self.refusal(), self.refusal()
        with self.assertRaises(ValueError) as caught:
            self.run_capacity(Mock(side_effect=[first, second]))
        self.assertIs(caught.exception, second)
        self.assertIs(self.record['existing_reclaim'], first.capacity)
        self.assertNotEqual(self.record.get('status'), 'ready')

    def test_exact_reserve_boundary_uses_integer_arithmetic(self):
        self.assertTrue(controller.enough(SimpleNamespace(total=1000, free=250), 100))
        self.assertFalse(controller.enough(SimpleNamespace(total=1000, free=249), 100))

    def test_public_entrypoint_refuses_local_and_self_hosted_execution(self):
        with patch.object(sys, 'argv', ['capacity']), patch.object(controller, 'query') as query:
            for environment in ({'CI': 'true', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'self-hosted'},
                                {'CI': 'false', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted'}):
                with patch.dict(os.environ, environment), self.assertRaisesRegex(ValueError, 'disposable hosted Mac'):
                    controller.main()
            query.assert_not_called()

    def test_failure_diagnostics_bind_source_and_keep_budget_and_helper_receipt(self):
        self.addCleanup(os.umask, os.umask(0))
        with tempfile.TemporaryDirectory() as directory:
            temp = Path(directory).resolve()
            root = temp / 'union-mac-123-1'
            receipts = root / 'inputs/receipts'
            receipts.mkdir(parents=True, mode=0o700)
            root.chmod(0o700)
            receipts.parent.chmod(0o700)
            fake = SimpleNamespace(cli_reclaim_xcode=lambda *args: None)
            original = fake.cli_reclaim_xcode
            error = self.refusal(growth=70 * 1024**3)
            fake.cli_prepare_ci_disk = Mock(side_effect=error)
            loader = SimpleNamespace(exec_module=lambda module: None)
            environment = {'CI': 'true', 'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
                           'RUNNER_TEMP': str(temp), 'OWNED_ROOT': str(root), 'INPUT_ARTIFACT': str(root / 'inputs'),
                           'GITHUB_RUN_ID': '123', 'GITHUB_RUN_ATTEMPT': '1', 'PREPARE_MODELS': 'true',
                           'SOURCE_COMMIT': 'a' * 40, 'SOURCE_TREE': 'b' * 40, 'GITHUB_SHA': 'c' * 40}
            def run(record, growth, prepare, measure):
                try:
                    prepare()
                except ValueError as failed:
                    record['existing_reclaim'] = failed.capacity
                    raise
            with patch.dict(os.environ, environment), patch.object(sys, 'argv', ['capacity']), \
                 patch.object(sys, 'platform', 'darwin'), patch.object(controller.pwd, 'getpwuid', return_value=SimpleNamespace(pw_name='runner')), \
                 patch.object(Path, 'is_relative_to', return_value=True), \
                 patch.object(controller, 'query', side_effect=['a' * 40, 'b' * 40, '']), \
                 patch.object(controller.importlib.util, 'spec_from_file_location', return_value=SimpleNamespace(loader=loader)), \
                 patch.object(controller.importlib.util, 'module_from_spec', return_value=fake), \
                 patch.object(controller, 'run_capacity', side_effect=run), \
                 patch.object(controller.shutil, 'disk_usage', return_value=SimpleNamespace(total=343073095680, free=85931712512)), \
                 self.assertRaises(ValueError):
                controller.main()
            self.assertIs(fake.cli_reclaim_xcode, original)
            capacity = json.loads((receipts / 'hosted-capacity.json').read_text())
            budget = json.loads((receipts / 'disk-budget.json').read_text())
            for name in ('hosted-capacity.json', 'disk-budget.json'):
                self.assertEqual((receipts / name).stat().st_mode & 0o777, 0o600)
            self.assertEqual(capacity['existing_reclaim'], error.capacity)
            self.assertEqual(budget['planned_growth_bytes'], 70 * 1024**3)
            self.assertEqual(budget['shortfall_bytes'], 40691179520)
            self.assertEqual(budget['reserve_percent'], 15)
            self.assertEqual(capacity['source_commit'], 'a' * 40)
            self.assertEqual(capacity['workflow_commit'], 'c' * 40)
            self.assertEqual(capacity['status'], 'unavailable')

    def test_native_binding_hashes_cover_tools_and_sdk_settings(self):
        with tempfile.TemporaryDirectory() as directory:
            developer = Path(directory).resolve() / 'Applications/Xcode.app/Contents/Developer'
            sdk = developer / 'SDKs/MacOSX.sdk'
            sdk.mkdir(parents=True)
            paths = {name: developer / name for name in ('clang', 'ld', 'simctl')}
            paths['sdk_settings'] = sdk / 'SDKSettings.json'
            for name, path in paths.items():
                path.write_bytes(name.encode())
            def query(argv, timeout=30):
                if argv[0] == '/usr/bin/xcode-select':
                    return str(developer)
                if argv[-1] == '--show-sdk-path':
                    return str(sdk)
                return str(paths[argv[-1]])
            with patch.object(Path, 'is_relative_to', return_value=True):
                record = controller.native_bindings(query)
            for name, path in paths.items():
                self.assertEqual(record[name + '_physical'], str(path))
                self.assertEqual(record[name + '_sha256'], hashlib.sha256(name.encode()).hexdigest())
            paths['clang'].write_bytes(b'changed')
            with patch.object(Path, 'is_relative_to', return_value=True):
                changed = controller.native_bindings(query)
            self.assertNotEqual(record, changed)
        with self.assertRaisesRegex(ValueError, 'must be absolute'):
            controller.native_bindings(lambda argv: 'relative')


if __name__ == '__main__':
    unittest.main()
