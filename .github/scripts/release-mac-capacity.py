#!/usr/bin/env python3
"""Reclaim disposable hosted Mac tools, preserving the native toolchain."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import pwd
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
    return (result.stdout + (result.stderr if argv[-2:] == ['help', 'runtime'] else '')).strip()


def native_bindings():
    bindings = {name: query(command) for name, command in {
        'xcode': ['/usr/bin/xcode-select', '-p'],
        'sdk': ['/usr/bin/xcrun', '--sdk', 'macosx', '--show-sdk-path'],
        'default_sdk': ['/usr/bin/xcrun', '--show-sdk-path'],
        **{name: ['/usr/bin/xcrun', '--sdk', 'macosx', '--find', name]
           for name in ('clang', 'ld', 'simctl')}}.items()}
    bindings['sdk_settings'] = str(Path(bindings['sdk']) / 'SDKSettings.json')
    if os.environ.get('DEVELOPER_DIR'):
        bindings['developer_dir'] = os.environ['DEVELOPER_DIR']
    for name, value in list(bindings.items()):
        path = Path(value)
        need(path.is_absolute(), 'Native binding must be absolute')
        path = path.resolve(strict=True)
        need(path.is_relative_to('/Applications'), 'Native binding ancestry differs')
        bindings[name] = str(path)
        if path.is_file():
            digest = hashlib.sha256()
            with path.open('rb') as stream:
                for block in iter(lambda: stream.read(1024**2), b''):
                    digest.update(block)
            bindings[name + '_sha256'] = digest.hexdigest()
    return bindings


def enough(disk, growth):
    return disk.free >= growth


def run_capacity(record, growth, prepare, measure, bindings=native_bindings,
                 query=query, monotonic=time.monotonic, sleep=time.sleep):
    record['native_before'] = bindings()
    try:
        record['reclaim'] = prepare()
    except ValueError as error:
        receipt = getattr(error, 'capacity', {})
        record['reclaim'] = receipt
        need(bindings() == record['native_before'], 'Native build inputs changed')
        if not (receipt.get('status') == 'unavailable' and receipt.get('planned_growth_bytes') == growth
                and all(type(receipt.get(key)) is int and receipt[key] > 0
                        for key in ('free_bytes_after', 'total_bytes'))
                and str(error) == f"hosted Mac capacity unavailable: free={receipt['free_bytes_after']} "
                    f"total={receipt['total_bytes']} planned_growth={growth}"
                and receipt['free_bytes_after'] < growth
                and not enough(measure(), growth)):
            raise
        help_text = query(['/usr/bin/xcrun', 'simctl', 'help', 'runtime'])
        need(all(word in help_text for word in ('delete', "'all'", 'list')), 'simctl runtime deletion unavailable')
        def inventory():
            images = json.loads(query(['/usr/bin/xcrun', 'simctl', 'runtime', 'list', '-j']))
            need(isinstance(images, dict), 'Simulator inventory differs')
            return images
        record['simulators_before'] = inventory()
        for operation in (['shutdown', 'all'], ['delete', 'all'], ['runtime', 'delete', 'all']):
            query(['/usr/bin/xcrun', 'simctl', *operation], timeout=60)
        deadline = monotonic() + 900
        while inventory() and not enough(measure(), growth):
            need(monotonic() < deadline, 'Simulator deletion timed out')
            sleep(1)
        need(bindings() == record['native_before'], 'Native build inputs changed')
        record['recheck'] = prepare()
    record['native_after'] = bindings()
    need(record['native_after'] == record['native_before'], 'Native build inputs changed')
    need(enough(measure(), growth), 'Capacity below planned growth')
    record['status'] = 'ready'


def main():
    need(len(sys.argv) == 1 and os.environ.get('CI') == 'true'
         and os.environ.get('GITHUB_ACTIONS') == 'true'
         and os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted' and sys.platform == 'darwin'
         and pwd.getpwuid(os.geteuid()).pw_name == 'runner', 'Requires a disposable hosted Mac')
    source, temp = Path.cwd().resolve(), Path(os.environ['RUNNER_TEMP'])
    root = Path(os.environ['RELEASE_ROOT'])
    need(source.is_relative_to('/Users/runner/work') and temp.is_relative_to('/Users/runner/work')
         and temp.resolve() == temp and root.parent == temp and root.resolve() == root
         and root.name == 'release-' + os.environ['GITHUB_RUN_ID'] + '-' + os.environ['GITHUB_RUN_ATTEMPT']
         and root.is_dir() and root.stat().st_uid == os.geteuid() and root.stat().st_mode & 0o077 == 0,
         'Hosted capacity root differs')
    need(query(['git', 'rev-parse', 'HEAD']) == os.environ['SOURCE_COMMIT']
         and query(['git', 'rev-parse', 'HEAD^{tree}']) == os.environ['SOURCE_TREE']
         and not query(['git', 'status', '--porcelain=v1', '--untracked-files=all']), 'Source binding differs')
    spec = importlib.util.spec_from_file_location('capacity', source / 'scripts/update-hop-compare.py')
    capacity = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(capacity)
    growth = 50 * 1024**3
    original = capacity.cli_reclaim_xcode
    def prepare():
        capacity.cli_reclaim_xcode = lambda apps, protected, ignored, measure, remove: original(apps, protected, growth, measure, remove)
        try:
            return capacity.cli_prepare_ci_disk()
        finally:
            capacity.cli_reclaim_xcode = original
    record = {'status': 'unavailable', 'source_commit': os.environ['SOURCE_COMMIT'],
              'source_tree': os.environ['SOURCE_TREE'], 'planned_growth_bytes': growth}
    try:
        run_capacity(record, growth, prepare, lambda: shutil.disk_usage(root))
    finally:
        disk = shutil.disk_usage(root)
        record.update(free_bytes=disk.free, total_bytes=disk.total)
        descriptor = os.open(root / 'capacity.json', os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        with os.fdopen(descriptor, 'w') as stream:
            stream.write(json.dumps(record, indent=2) + '\n')


if __name__ == '__main__':
    main()
