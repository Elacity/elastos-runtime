#!/usr/bin/env python3
"""Read inert release archives and seal receipts on a fresh trusted builder job.

The workflow passes original H, admitted tree and W. Candidate build jobs own
payload bytes; this helper owns receipt assertions and runs no payload code.
The same bounded unpacker is used by the key-free operator consumer.
"""
import argparse
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import zipfile

spec = importlib.util.spec_from_file_location('release_signer', Path(__file__).with_name('release-signer.py'))
S = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = S
spec.loader.exec_module(S)
LIMIT = 64 * 1024**3
MEMBER = 8 * 1024**3
COUNT = 8192


def name_parts(name):
    S.require(type(name) is str and len(name) <= 512 and '\\' not in name
              and all(ord(char) >= 32 and ord(char) != 127 for char in name)
              and all(p not in ('', '.', '..') for p in name.split('/')),
              'unsafe archive member path')
    return name.split('/')


def empty_root(path):
    S.require(not path.exists() and not path.is_symlink(), 'fresh extraction directory required')
    path.mkdir(mode=0o700, parents=True)


def digest_file(path):
    digest = hashlib.sha256()
    fd = S.file_fd(path.absolute())
    with os.fdopen(fd, 'rb') as source:
        S.require(stat.S_ISREG(os.fstat(source.fileno()).st_mode), 'regular archive payload required')
        for chunk in iter(lambda: source.read(1024 * 1024), b''):
            digest.update(chunk)
    return digest.hexdigest()


def unzip(path, output):
    empty_root(output)
    seen = set()
    total = 0
    with zipfile.ZipFile(path) as archive:
        S.require(len(archive.infolist()) <= COUNT, 'ZIP member count refused')
        for entry in archive.infolist():
            parts = name_parts(entry.filename)
            # GitHub release artifacts have a flat regular-file package.
            mode = entry.external_attr >> 16
            S.require(len(parts) == 1 and entry.filename not in seen and not entry.is_dir()
                      and (stat.S_IFMT(mode) in (0, stat.S_IFREG)) and not entry.flag_bits & 1,
                      'ZIP requires unique flat regular files')
            S.require(0 <= entry.file_size <= MEMBER, 'ZIP member size refused')
            total += entry.file_size
            S.require(total <= LIMIT, 'ZIP total size refused')
            seen.add(entry.filename)
            with archive.open(entry) as source, (output / entry.filename).open('xb') as target:
                copy_payload(source, target, entry.file_size)
            (output / entry.filename).chmod(0o700 if mode & 0o111 else 0o600)


def checksums(root):
    data = S.regular_bytes(root / 'SHA256SUMS', S.MAX_JSON).decode('ascii')
    seen = set()
    for line in data.splitlines():
        match = re.fullmatch(r'([0-9a-f]{64}) [ *](.+)', line)
        S.require(match is not None, 'checksum record refused')
        name = match[2].removeprefix('./')
        name_parts(name)
        S.require(name not in seen and name != 'SHA256SUMS' and len(seen) < COUNT, 'checksum inventory refused')
        seen.add(name)
        S.require(digest_file(root / name) == match[1], 'archive checksum differs')
    S.require(bool(seen), 'checksum inventory required')


def copy_payload(source, target, size):
    digest = hashlib.sha256()
    captured = bytearray() if size <= S.MAX_JSON else None
    remaining = size
    while remaining:
        data = source.read(min(remaining, 1024 * 1024))
        S.require(bool(data), 'truncated archive payload')
        remaining -= len(data)
        digest.update(data)
        if target is not None:
            target.write(data)
        if captured is not None:
            captured.extend(data)
    S.require(not source.read(1), 'archive payload exceeds declared size')
    return digest.hexdigest(), bytes(captured) if captured is not None else None


class Parts(io.RawIOBase):
    def __init__(self, paths):
        self.paths = iter(paths)
        self.source = None

    def readable(self):
        return True

    def readinto(self, buffer):
        while True:
            if self.source is None:
                try:
                    self.source = os.fdopen(S.file_fd(next(self.paths).absolute()), 'rb')
                except StopIteration:
                    return 0
            count = self.source.readinto(buffer)
            if count:
                return count
            self.source.close()
            self.source = None

    def close(self):
        if self.source is not None:
            self.source.close()
        super().close()


class BoundedTarInfo(tarfile.TarInfo):
    def metadata_limit(self, archive, maximum):
        S.require(0 <= self.size <= maximum, 'tar metadata size refused')
        total = getattr(archive, '_release_metadata_bytes', 0) + self.size
        S.require(total <= 16 * S.MAX_JSON, 'tar metadata total size refused')
        archive._release_metadata_bytes = total

    def _proc_sparse(self, archive):
        raise ValueError('sparse archive entries refused')

    def _proc_gnusparse_00(self, *args):
        raise ValueError('sparse archive entries refused')

    def _proc_gnusparse_01(self, *args):
        raise ValueError('sparse archive entries refused')

    def _proc_gnusparse_10(self, *args):
        raise ValueError('sparse archive entries refused')

    def _proc_pax(self, archive):
        self.metadata_limit(archive, S.MAX_JSON)
        return super()._proc_pax(archive)

    def _proc_gnulong(self, archive):
        self.metadata_limit(archive, 1024)
        return super()._proc_gnulong(archive)


def tar_inputs(package, output=None):
    paths = sorted(package.glob('release-inputs.tar.gz.part*'))
    S.require(paths and all(re.fullmatch(r'release-inputs\.tar\.gz\.part[a-z]{3}', p.name) for p in paths),
              'release archive parts required')
    S.require(len(paths) <= COUNT and sum(p.stat().st_size for p in paths) <= LIMIT, 'archive parts size refused')
    if output is not None:
        empty_root(output)
    seen, records, receipts = set(), {}, {}
    total = 0
    with Parts(paths) as parts, io.BufferedReader(parts) as stream, tarfile.open(fileobj=stream, mode='r|gz', tarinfo=BoundedTarInfo) as archive:
        for entry in archive:
            name = entry.name.rstrip('/')
            components = name_parts(name)
            S.require(components[0] == 'inputs' and name not in seen and len(seen) < COUNT,
                      'release tar path or duplicate refused')
            seen.add(name)
            S.require(entry.isdir() or entry.isreg(), 'release tar requires regular files and directories')
            if entry.isdir():
                if output is not None and len(components) > 1:
                    (output / Path(*components[1:])).mkdir(mode=0o700, parents=True, exist_ok=True)
                continue
            S.require(len(components) > 1 and 0 <= entry.size <= MEMBER, 'release tar member size refused')
            total += entry.size
            S.require(total <= LIMIT, 'release tar total size refused')
            relative = '/'.join(components[1:])
            target = None
            if output is not None:
                path = output / Path(*components[1:])
                path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
                target = path.open('xb')
            with archive.extractfile(entry) as source:
                try:
                    digest, data = copy_payload(source, target, entry.size)
                finally:
                    if target is not None:
                        target.close()
            if output is not None:
                path.chmod(0o700 if entry.mode & 0o111 else 0o600)
            records[relative] = {'sha256': digest, 'size': entry.size, 'executable': bool(entry.mode & 0o111)}
            if relative in ('N/platform-input.json', 'N1/platform-input.json'):
                S.require(data is not None, 'platform receipt size refused')
                receipts[relative.split('/')[0]] = S.parse_json(data)
    return records, receipts


def seal(package, output, source, platform, versions, producer):
    empty_root(output)
    if platform == 'seed':
        receipt = S.parse_json(S.regular_bytes(package / 'seed-input.json', S.MAX_JSON))
        S.require(receipt.get('schema') == 'elastos.release-seed-input/v1' and receipt.get('source') == source
                  and receipt.get('version') == versions[0] and set(receipt.get('files', {})) == {'elastos', 'ipfs-provider', 'source.tar.gz'},
                  'seed receipt differs from original workflow input')
        for name, record in receipt['files'].items():
            path = package / name
            S.require(record == {'sha256': digest_file(path), 'size': path.stat().st_size}, 'seed receipt payload differs')
        receipt['producer'] = producer
        (output / 'seed-input.json').write_bytes(S.json_bytes(receipt))
    else:
        records, receipts = tar_inputs(package)
        S.require(set(receipts) == {'N', 'N1'}, 'both platform receipts required')
        for name, version in zip(('N', 'N1'), versions):
            receipt = receipts[name]
            S.require(receipt.get('schema') == 'elastos.release-platform-input/v1' and receipt.get('platform') == platform
                      and receipt.get('version') == version and receipt.get('source', {}).get('clean') is True
                      and {k: receipt.get('source', {}).get(k) for k in source} == source,
                      'platform receipt differs from original workflow input')
            actual = {path[len(name) + 1:]: value for path, value in records.items()
                      if path.startswith(name + '/') and path != name + '/platform-input.json'}
            S.require(receipt.get('files') == actual, 'platform receipt payload inventory differs')
            receipt['producer'] = producer
            (output / (name + '.platform-input.json')).write_bytes(S.json_bytes(receipt))


def api(path):
    result = subprocess.run(['gh', 'api', '--hostname', 'github.com', path], capture_output=True, timeout=30)
    S.require(result.returncode == 0 and len(result.stdout) <= 8 * S.MAX_JSON, 'attester GitHub authority unavailable')
    return S.parse_json(result.stdout)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('command', choices=('unzip', 'untar', 'checksums', 'attest'))
    parser.add_argument('--input', type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--source-commit')
    parser.add_argument('--source-tree')
    parser.add_argument('--workflow-commit')
    parser.add_argument('--workflow-ref')
    parser.add_argument('--run', type=int)
    parser.add_argument('--attempt', type=int)
    parser.add_argument('--platform')
    parser.add_argument('--prefix')
    parser.add_argument('--install-version')
    parser.add_argument('--update-version')
    args = parser.parse_args()
    if args.command == 'unzip':
        unzip(args.input.absolute(), args.output.absolute())
    elif args.command == 'checksums':
        checksums(args.input.absolute())
    elif args.command == 'untar':
        tar_inputs(args.input.absolute(), args.output.absolute())
    else:
        S.require(all(type(value) is str and re.fullmatch(r'[0-9a-f]{40}', value) for value in
                      (args.source_commit, args.source_tree, args.workflow_commit)), 'full source/tree/workflow SHAs required')
        source = {'commit': args.source_commit, 'tree': args.source_tree}
        workflow = args.workflow_commit
        S.require(args.workflow_ref == S.PACKAGE_REF and args.run > 0 and args.attempt > 0,
                  'trusted workflow invocation required')
        run = api(f'/repos/{S.REPOSITORY}/actions/runs/{args.run}')
        S.require(run.get('id') == args.run and run.get('run_attempt') == args.attempt and run.get('head_sha') == workflow
                  and run.get('head_branch') == args.workflow_ref[11:] and run.get('path') == S.PACKAGE_WORKFLOW
                  and run.get('event') == 'workflow_dispatch' and run.get('repository', {}).get('full_name') == S.REPOSITORY
                  and run.get('head_repository', {}).get('full_name') == S.REPOSITORY,
                  'attester workflow invocation differs')
        artifact = S.package_artifact({'source': source, 'workflow': {'commit': workflow}}, run, args.prefix, api)
        staging = args.output.absolute().parent / ('native-' + args.platform)
        staging.mkdir(mode=0o700, parents=True)
        archive = staging / 'input.zip'
        with archive.open('xb') as target:
            result = subprocess.run(['gh', 'api', '--hostname', 'github.com',
                                     f'/repos/{S.REPOSITORY}/actions/artifacts/{artifact["id"]}/zip'], stdout=target, timeout=600)
        S.require(result.returncode == 0 and archive.stat().st_size <= LIMIT and 'sha256:' + digest_file(archive) == artifact['digest'],
                  'native artifact ZIP differs')
        package = staging / 'package'
        unzip(archive, package)
        producer = {'artifact': artifact, 'run_id': args.run, 'workflow_commit': workflow, 'attestation_attempt': args.attempt}
        seal(package, args.output.absolute(), source, args.platform, (args.install_version, args.update_version), producer)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError, tarfile.TarError, zipfile.BadZipFile, KeyError, TypeError) as error:
        sys.exit(f'release-provenance-seal: {error}')
