#!/usr/bin/env python3
"""Reclaim disposable hosted Mac tools while preserving native build inputs."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
import re
import shutil
import subprocess
import sys
import time

sys.dont_write_bytecode = True


def need(condition, message):
    if not condition:
        raise ValueError(message)


def query(argv, timeout=30):
    result = subprocess.run(argv, capture_output=True, text=True, check=True, timeout=timeout)
    if argv == ['/usr/bin/xcrun', 'simctl', 'help', 'runtime']:
        return (result.stdout + result.stderr).strip()
    return result.stdout.strip()


def native_bindings(query=query):
    selected = query(['/usr/bin/xcode-select', '-p'])
    sdk = query(['/usr/bin/xcrun', '--sdk', 'macosx', '--show-sdk-path'])
    records = {'selected_xcode': selected, 'default_sdk': query(['/usr/bin/xcrun', '--show-sdk-path']),
               'macos_sdk': sdk, 'developer_dir': os.environ.get('DEVELOPER_DIR', '')}
    for name, value in [('selected_xcode', selected), ('macos_sdk', sdk),
                        ('default_sdk', records['default_sdk'])]:
        need(Path(value).is_absolute(), 'Native Xcode binding must be absolute')
        path = Path(value).resolve(strict=True)
        need(path.is_dir() and path.is_relative_to('/Applications'), 'Native Xcode ancestry differs')
        records[name + '_physical'] = str(path)
    for name in ('clang', 'ld', 'simctl'):
        records[name] = query(['/usr/bin/xcrun', '--sdk', 'macosx', '--find', name])
    records['sdk_settings'] = str(Path(sdk) / 'SDKSettings.json')
    for name in ('clang', 'ld', 'simctl', 'sdk_settings'):
        need(Path(records[name]).is_absolute(), 'Native tool binding must be absolute')
        path = Path(records[name]).resolve(strict=True)
        need(path.is_file() and path.is_relative_to('/Applications'), 'Native tool ancestry differs')
        digest = hashlib.sha256()
        with path.open('rb') as stream:
            for block in iter(lambda: stream.read(1024**2), b''):
                digest.update(block)
        records[name + '_physical'] = str(path)
        records[name + '_sha256'] = digest.hexdigest()
    return records


def enough(disk, growth):
    return (disk.free - growth) * 100 >= disk.total * 15


def run_capacity(record, growth, prepare_existing, measure, bindings=native_bindings,
                 query=query, monotonic=time.monotonic, sleep=time.sleep):
    record['native_before'] = bindings()
    record['phase'] = 'existing-tool-reclaim'
    try:
        record['existing_reclaim'] = prepare_existing()
    except Exception as error:
        receipt = getattr(error, 'capacity', None)
        if isinstance(receipt, dict):
            record['existing_reclaim'] = receipt
        after = bindings()
        record['native_after_existing'] = after
        if not (isinstance(error, ValueError) and isinstance(receipt, dict)
                and receipt.get('status') == 'unavailable'
                and receipt.get('planned_growth_bytes') == growth
                and all(type(receipt.get(key)) is int and receipt[key] > 0
                        for key in ('free_bytes_after', 'total_bytes'))
                and str(error) == 'hosted Mac capacity unavailable: free=' + str(receipt['free_bytes_after'])
                    + ' total=' + str(receipt['total_bytes']) + ' planned_growth=' + str(growth)
                and (receipt['free_bytes_after'] - growth) * 100 < receipt['total_bytes'] * 15
                and after == record['native_before'] and not enough(measure(), growth)):
            raise
        record['phase'] = 'simulator-reclaim'
        help_text = query(['/usr/bin/xcrun', 'simctl', 'help', 'runtime'])
        need('delete' in help_text and "'all'" in help_text and 'list' in help_text,
             'Hosted simctl runtime removal is unavailable')
        images = json.loads(query(['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']))
        need(isinstance(images, dict), 'Simulator image inventory differs')
        record['simulator_images_before'] = images
        record['simulator_free_bytes_before'] = measure().free
        for operation in (['shutdown', 'all'], ['delete', 'all'], ['runtime', 'delete', 'all']):
            query(['/usr/bin/xcrun', 'simctl', *operation], timeout=60)
        # Asset-backed runtime deletion continues after simctl returns.
        deadline = monotonic() + 300
        while True:
            images = json.loads(query(['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']))
            need(isinstance(images, dict), 'Simulator image inventory differs')
            record['simulator_images_after'] = images
            if not images:
                break
            need(monotonic() < deadline, 'Simulator runtime removal timed out')
            sleep(1)
        record['simulator_free_bytes_after'] = measure().free
        record['phase'] = 'capacity-recheck'
        record['native_after_simulators'] = bindings()
        need(record['native_after_simulators'] == record['native_before'], 'Native build inputs changed')
        try:
            record['recheck_reclaim'] = prepare_existing()
        except Exception as error:
            receipt = getattr(error, 'capacity', None)
            if isinstance(receipt, dict):
                record['recheck_reclaim'] = receipt
            raise
    record['native_after'] = bindings()
    need(record['native_after'] == record['native_before'], 'Native build inputs changed')
    need(enough(measure(), growth), 'Hosted Mac capacity remains below planned growth and 15% reserve')
    record.update(status='ready', phase='complete')


def main():
    need(len(sys.argv) == 1, 'Capacity controller uses fixed workflow inputs')
    need(os.environ.get('CI') == 'true' and os.environ.get('GITHUB_ACTIONS') == 'true'
         and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted' and sys.platform == 'darwin'
         and pwd.getpwuid(os.geteuid()).pw_name == 'runner', 'Capacity reclaim requires a disposable hosted Mac runner')
    source = Path.cwd().resolve()
    temp = Path(os.environ['RUNNER_TEMP'])
    root, artifact = Path(os.environ['OWNED_ROOT']), Path(os.environ['INPUT_ARTIFACT'])
    need(source.is_relative_to('/Users/runner/work') and temp.is_absolute() and temp.is_dir()
         and temp.resolve() == temp and temp.is_relative_to('/Users/runner/work')
         and root.parent == temp and root.name == 'union-mac-' + os.environ['GITHUB_RUN_ID'] + '-' + os.environ['GITHUB_RUN_ATTEMPT']
         and root.resolve() == root and root.is_dir() and artifact == root / 'inputs'
         and artifact.resolve() == artifact and artifact.is_dir(), 'Hosted capacity path ancestry differs')
    need(all(path.stat().st_uid == os.geteuid() and path.stat().st_mode & 0o077 == 0
             for path in (root, artifact)), 'Capacity root requires owned protected directories')
    receipts = artifact / 'receipts'
    need(receipts.is_dir() and receipts.resolve() == receipts and receipts.stat().st_uid == os.geteuid()
         and receipts.stat().st_mode & 0o077 == 0, 'Capacity receipts require an owned protected directory')
    need(os.environ['PREPARE_MODELS'] in ('true', 'false'), 'Model preparation mode differs')
    growth = (70 if os.environ['PREPARE_MODELS'] == 'true' else 50) * 1024**3
    record = {'schema': 'elastos.canary-mac-capacity/v1', 'status': 'unavailable', 'phase': 'source-binding',
              'source_commit': os.environ['SOURCE_COMMIT'], 'source_tree': os.environ['SOURCE_TREE'],
              'workflow_commit': os.environ['GITHUB_SHA'], 'planned_growth_bytes': growth, 'reserve_percent': 15}
    measure = lambda: shutil.disk_usage(root)
    try:
        need(all(re.fullmatch('[0-9a-f]{40}', record[key]) for key in ('source_commit', 'source_tree', 'workflow_commit'))
             and query(['git', 'rev-parse', 'HEAD']) == record['source_commit']
             and query(['git', 'rev-parse', 'HEAD^{tree}']) == record['source_tree']
             and not query(['git', 'status', '--porcelain=v1', '--untracked-files=all']), 'Admitted source binding differs')
        spec = importlib.util.spec_from_file_location('capacity', source / 'scripts/update-hop-compare.py')
        capacity = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(capacity)
        original = capacity.cli_reclaim_xcode

        def prepare_existing():
            capacity.cli_reclaim_xcode = lambda apps, protected, ignored, measure, remove: original(apps, protected, growth, measure, remove)
            try:
                return capacity.cli_prepare_ci_disk()
            finally:
                capacity.cli_reclaim_xcode = original

        run_capacity(record, growth, prepare_existing, measure)
    except Exception as error:
        record.update(status='unavailable', error_type=type(error).__name__)
        if 'native_before' in record:
            try:
                record['native_after_failure'] = native_bindings()
                record['native_inputs_preserved'] = record['native_after_failure'] == record['native_before']
            except Exception as binding_error:
                record['native_binding_error_type'] = type(binding_error).__name__
        raise
    finally:
        disk = measure()
        budget = {key: record[key] for key in ('source_commit', 'source_tree', 'workflow_commit', 'planned_growth_bytes', 'reserve_percent')}
        reserve = (disk.total * 15 + 99) // 100
        budget.update(free_bytes=disk.free, total_bytes=disk.total, required_reserve_bytes=reserve,
                      shortfall_bytes=max(0, growth + reserve - disk.free), status=record['status'])
        for name, data in [('hosted-capacity.json', record), ('disk-budget.json', budget)]:
            path = receipts / name
            need(not path.is_symlink(), 'Capacity diagnostic path is a symlink')
            descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC | os.O_NOFOLLOW, 0o600)
            with os.fdopen(descriptor, 'w') as stream:
                os.fchmod(stream.fileno(), 0o600)
                stream.write(json.dumps(data, indent=2) + '\n')


if __name__ == '__main__':
    main()
