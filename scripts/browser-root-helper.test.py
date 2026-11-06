#!/usr/bin/env python3
"""Exercise Runtime's actual privileged bootstrap without granting root."""
import hashlib
import importlib.util
import io
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch
import sys

SOURCE = (Path(__file__).resolve().parents[1] / 'elastos/crates/elastos-server/src/setup'
          / 'browser_linux_root.py')
SPEC = importlib.util.spec_from_file_location('browser_linux_root', SOURCE)
ROOT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(ROOT)


class RootHelperTests(unittest.TestCase):
    def test_setup_keeps_the_helper_confirmation_on_the_tty(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
            self.addCleanup(os.close, directory)
            payload = b"assert input('Continue? [y/N] ') == 'y'\n"
            expected = hashlib.sha256(payload).hexdigest()
            verified_copy = ROOT.verified_copy
            # Model root's ownership with the fixture account; the real copy,
            # read-back digest and execution path all run unchanged.
            def copy_as_fixture(fd, name, digest, contents, owner):
                self.assertEqual(owner, 0)
                self.assertEqual(digest, expected)
                return verified_copy(fd, name, digest, contents, os.getuid())
            with (patch.object(ROOT.os, 'geteuid', return_value=0),
                  patch.object(ROOT, 'trusted_directory', return_value=os.dup(directory)),
                  patch.object(ROOT, 'verified_copy', side_effect=copy_as_fixture),
                  patch.object(sys, 'argv', ['bootstrap', expected, 'fixture', temporary, 'setup']),
                  patch.object(sys, 'stdin', io.TextIOWrapper(io.BytesIO(payload))),
                  patch('builtins.open', return_value=io.StringIO('y\n')) as tty,
                  patch.object(sys, 'stdout', io.StringIO()) as output):
                ROOT.main()
                tty.assert_called_once_with('/dev/tty', encoding='utf-8')
                self.assertIn('Continue? [y/N]', output.getvalue())

    def test_copy_accepts_only_admitted_bytes_and_replaces_stale_root_copy(self):
        for existing in (None, b'stale root helper'):
            with self.subTest(existing=existing), tempfile.TemporaryDirectory() as temporary:
                path = Path(temporary) / 'helper.py'
                if existing is not None:
                    path.write_bytes(existing)
                directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
                self.addCleanup(os.close, directory)
                payload = b'print("admitted")\n'
                expected = hashlib.sha256(payload).hexdigest()
                verified = ROOT.verified_copy(directory, path.name, expected, payload, os.getuid())
                self.assertEqual(verified, payload)
                self.assertEqual(path.read_bytes(), payload)
                self.assertEqual(path.stat().st_mode & 0o777, 0o644)

    def test_changed_payload_is_refused_before_replacing_or_executing_root_copy(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'helper.py'
            path.write_bytes(b'previous admitted root helper')
            directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
            self.addCleanup(os.close, directory)
            expected = hashlib.sha256(b'admitted').hexdigest()
            with self.assertRaisesRegex(RuntimeError, 'admitted SHA-256'):
                ROOT.verified_copy(directory, path.name, expected, b'changed!', os.getuid())
            self.assertEqual(path.read_bytes(), b'previous admitted root helper')

    def test_existing_symlink_is_replaced_without_reading_or_writing_its_target(self):
        with tempfile.TemporaryDirectory() as temporary:
            target = Path(temporary) / 'outside'
            target.write_bytes(b'keep')
            path = Path(temporary) / 'helper.py'
            path.symlink_to(target)
            directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
            self.addCleanup(os.close, directory)
            payload = b'admitted'
            ROOT.verified_copy(directory, path.name, hashlib.sha256(payload).hexdigest(),
                               payload, os.getuid())
            self.assertFalse(path.is_symlink())
            self.assertEqual(path.read_bytes(), payload)
            self.assertEqual(target.read_bytes(), b'keep')

    def test_writable_or_foreign_directory_is_refused(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
            self.addCleanup(os.close, directory)
            payload = b'admitted'
            expected = hashlib.sha256(payload).hexdigest()
            with self.assertRaisesRegex(RuntimeError, 'root-owned directory'):
                ROOT.verified_copy(directory, 'helper.py', expected, payload, os.getuid() + 1)
            os.chmod(temporary, 0o777)
            with self.assertRaisesRegex(RuntimeError, 'root-owned directory'):
                ROOT.verified_copy(directory, 'helper.py', expected, payload, os.getuid())

    def test_root_copy_is_reverified_before_any_helper_execution(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = os.open(temporary, os.O_RDONLY | os.O_DIRECTORY)
            self.addCleanup(os.close, directory)
            payload = b'admitted'
            expected = hashlib.sha256(payload).hexdigest()
            rename = os.rename
            def corrupt_after_write(*args, **kwargs):
                rename(*args, **kwargs)
                (Path(temporary) / 'helper.py').write_bytes(b'changed!')
            with patch.object(ROOT.os, 'rename', side_effect=corrupt_after_write):
                with self.assertRaisesRegex(RuntimeError, 'Root Browser helper differs'):
                    ROOT.verified_copy(directory, 'helper.py', expected, payload, os.getuid())

    def test_directory_walk_refuses_symlink(self):
        with tempfile.TemporaryDirectory() as temporary:
            link = Path(temporary) / 'link'
            link.symlink_to(temporary)
            # Start from this process's trusted fixture directory, so the test
            # works on macOS (/var is a symlink) and Linux alike.
            real_open = os.open
            def fixture_open(path, *args, **kwargs):
                return real_open(temporary if path == '/' else path, *args, **kwargs)
            with patch.object(ROOT.os, 'open', side_effect=fixture_open):
                with self.assertRaises(OSError):
                    ROOT.trusted_directory('/link', os.getuid())


if __name__ == '__main__':
    unittest.main()
