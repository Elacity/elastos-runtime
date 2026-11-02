#!/usr/bin/env python3
"""Small inert archive and fresh-attester boundary regression fixtures."""
import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import stat
import sys
import tarfile
import tempfile
import unittest
from unittest import mock
import zipfile

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('release_seal', ROOT / 'release-provenance-seal.py')
M = importlib.util.module_from_spec(spec)
spec.loader.exec_module(M)
H, TREE, W = 'a' * 40, 'b' * 40, 'c' * 40
SOURCE = {'commit': H, 'tree': TREE}
PRODUCER = {'artifact': {'id': 1, 'name': f'release-mac-{H}-1', 'digest': 'sha256:' + 'd' * 64, 'attempt': 1},
            'run_id': 123, 'workflow_commit': W, 'attestation_attempt': 2}


class SealTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.package = self.root / 'package'
        self.package.mkdir()

    def archive(self, source=None, extra=None):
        path = self.package / 'release-inputs.tar.gz.partaaa'
        with tarfile.open(path, 'w:gz') as archive:
            for name, version in (('N', '1.2.3'), ('N1', '1.2.4')):
                # If accidentally executed, this payload would create a marker.
                payload = b'#!/bin/sh\ntouch SHOULD_NEVER_EXECUTE\n'
                record = {'sha256': hashlib.sha256(payload).hexdigest(), 'size': len(payload), 'executable': True}
                receipt = {'schema': 'elastos.release-platform-input/v1', 'platform': 'aarch64-darwin', 'version': version,
                           'source': {**(source or SOURCE), 'clean': True}, 'files': {'artifacts/elastos-aarch64-darwin': record}}
                for relative, data, mode in ((name + '/artifacts/elastos-aarch64-darwin', payload, 0o755),
                                             (name + '/platform-input.json', json.dumps(receipt).encode(), 0o600)):
                    entry = tarfile.TarInfo('inputs/' + relative)
                    entry.size, entry.mode = len(data), mode
                    archive.addfile(entry, io.BytesIO(data))
            if extra:
                entry, data = extra
                archive.addfile(entry, io.BytesIO(data) if entry.isreg() else None)
        return path

    def test_seals_original_h_and_mixed_attempt_without_executing_candidate(self):
        self.archive()
        output = self.root / 'proof'
        M.seal(self.package, output, SOURCE, 'aarch64-darwin', ('1.2.3', '1.2.4'), PRODUCER)
        receipt = json.loads((output / 'N.platform-input.json').read_bytes())
        self.assertEqual(receipt['producer'], PRODUCER)
        self.assertEqual(receipt['source']['commit'], H)
        self.assertFalse((self.root / 'SHOULD_NEVER_EXECUTE').exists())
        self.assertFalse((Path.cwd() / 'SHOULD_NEVER_EXECUTE').exists())

    def test_candidate_claiming_different_h_cannot_receive_original_input_receipt(self):
        for field in ('commit', 'tree'):
            with self.subTest(field=field):
                self.archive({**SOURCE, field: 'f' * 40})
                with self.assertRaisesRegex(ValueError, 'original workflow input'):
                    M.seal(self.package, self.root / ('proof-' + field), SOURCE, 'aarch64-darwin', ('1.2.3', '1.2.4'), PRODUCER)

    def test_payload_inventory_cannot_be_forged(self):
        entry = tarfile.TarInfo('inputs/N/artifacts/unlisted')
        entry.size = 1
        self.archive(extra=(entry, b'x'))
        with self.assertRaisesRegex(ValueError, 'inventory'):
            M.seal(self.package, self.root / 'proof', SOURCE, 'aarch64-darwin', ('1.2.3', '1.2.4'), PRODUCER)

    def test_tar_refuses_traversal_absolute_duplicate_links_and_devices(self):
        for name, kind in (('inputs/../escaped', tarfile.REGTYPE), ('/escaped', tarfile.REGTYPE),
                           ('inputs/N/platform-input.json', tarfile.REGTYPE), ('inputs/link', tarfile.SYMTYPE),
                           ('inputs/hard', tarfile.LNKTYPE), ('inputs/device', tarfile.CHRTYPE)):
            with self.subTest(name=name):
                entry = tarfile.TarInfo(name)
                entry.type, entry.linkname = kind, '../../escaped'
                entry.size = 1 if entry.isreg() else 0
                self.archive(extra=(entry, b'x'))
                with self.assertRaises(ValueError):
                    M.tar_inputs(self.package, self.root / ('extract-' + str(len(list(self.root.iterdir())))))
                self.assertFalse((self.root / 'escaped').exists())

    def test_sparse_headers_and_sparse_metadata_are_refused_before_parsing(self):
        entry = tarfile.TarInfo('inputs/sparse')
        entry.type = tarfile.GNUTYPE_SPARSE
        self.archive(extra=(entry, b''))
        with self.assertRaisesRegex(ValueError, 'sparse'):
            M.tar_inputs(self.package)
        bounded = M.BoundedTarInfo('sparse')
        for name in ('_proc_gnusparse_00', '_proc_gnusparse_01', '_proc_gnusparse_10'):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, 'sparse'):
                getattr(bounded, name)(None, None)

    def test_tar_size_bound_and_fresh_destination(self):
        self.archive()
        with mock.patch.object(M, 'LIMIT', 20):
            with self.assertRaisesRegex(ValueError, 'size'):
                M.tar_inputs(self.package)
        with self.assertRaisesRegex(ValueError, 'fresh extraction'):
            M.tar_inputs(self.package, self.package)

    def test_zip_refuses_links_traversal_nested_and_duplicate_entries(self):
        for name, mode in (('../escaped', stat.S_IFREG), ('/escaped', stat.S_IFREG), ('nested/file', stat.S_IFREG),
                           ('link', stat.S_IFLNK), ('device', stat.S_IFCHR)):
            with self.subTest(name=name):
                path = self.root / 'input.zip'
                entry = zipfile.ZipInfo(name)
                entry.external_attr = (mode | 0o600) << 16
                with zipfile.ZipFile(path, 'w') as archive:
                    archive.writestr(entry, b'../../escaped')
                with self.assertRaises(ValueError):
                    M.unzip(path, self.root / ('zip-' + str(len(list(self.root.iterdir())))))
                self.assertFalse((self.root / 'escaped').exists())

    def test_checksum_manifest_paths_stay_inside_owned_package(self):
        payload = b'inert'
        (self.package / 'payload').write_bytes(payload)
        digest = hashlib.sha256(payload).hexdigest()
        manifest = self.package / 'SHA256SUMS'
        manifest.write_text(digest + '  ./payload\n')
        M.checksums(self.package)
        for name in ('../escaped', '/escaped', 'payload\x01'):
            manifest.write_text(digest + '  ' + name + '\n')
            with self.subTest(name=name), self.assertRaises(ValueError):
                M.checksums(self.package)

    def test_artifact_name_for_a_foreign_h_has_no_original_input_proof(self):
        run = {'id': 123, 'run_attempt': 2, 'head_branch': 'develop'}
        foreign = {'id': 1, 'name': 'release-mac-' + 'f' * 40 + '-1', 'expired': False}
        with self.assertRaisesRegex(ValueError, 'live artifact'):
            M.S.package_artifact({'source': SOURCE, 'workflow': {'commit': W}}, run, 'release-mac',
                                 lambda path: {'artifacts': [foreign]})

    def test_seed_receipt_hashes_actual_files_and_original_source(self):
        files = {}
        for name in ('elastos', 'ipfs-provider', 'source.tar.gz'):
            data = b'inert ' + name.encode()
            (self.package / name).write_bytes(data)
            files[name] = {'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data)}
        receipt = {'schema': 'elastos.release-seed-input/v1', 'source': SOURCE, 'version': '1.2.3', 'files': files}
        (self.package / 'seed-input.json').write_text(json.dumps(receipt))
        M.seal(self.package, self.root / 'proof', SOURCE, 'seed', ('1.2.3', '1.2.4'), PRODUCER)
        self.assertEqual(json.loads((self.root / 'proof/seed-input.json').read_bytes())['producer'], PRODUCER)
        (self.package / 'elastos').write_bytes(b'changed')
        with self.assertRaisesRegex(ValueError, 'payload differs'):
            M.seal(self.package, self.root / 'changed', SOURCE, 'seed', ('1.2.3', '1.2.4'), PRODUCER)

    def test_workflow_attestation_authority_is_only_on_fresh_w_jobs(self):
        text = (ROOT.parent / '.github/workflows/release-package.yml').read_text()
        builds, fresh = text.split('\n  attest-platform:', 1)
        self.assertNotIn('id-token: write', builds)
        self.assertNotIn('attestations: write', builds)
        self.assertNotIn('attest-build-provenance@', builds)
        self.assertIn('ref: ${{ github.sha }}', fresh)
        self.assertIn('SOURCE_TREE: ${{ needs.admit.outputs.source_tree }}', fresh)
        self.assertIn('--source-commit "$SOURCE_COMMIT"', fresh)
        self.assertNotIn('ref: ${{ inputs.source_commit }}', fresh)
        self.assertNotIn('--version 2>&1', fresh)
        self.assertNotIn('prepare-release-platform.sh', fresh)


if __name__ == '__main__':
    unittest.main()
