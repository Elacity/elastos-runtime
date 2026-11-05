#!/usr/bin/env python3
"""Python/shell removal helper fixtures from reviewed 616a8af6e."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
HELPER = SOURCE / "lib/owned_removal.py"
spec = importlib.util.spec_from_file_location("owned_removal", HELPER)
owned_removal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owned_removal)



class OwnedRemovalTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.scratch = Path(scratch.name).resolve()
        self.root, self.outside = self.scratch / "root", self.scratch / "outside"
        (self.root / "owned/nested").mkdir(parents=True)
        self.outside.mkdir()
        (self.root / "owned/file").write_text("owned")
        (self.root / "owned/nested/file").write_text("owned")
        (self.outside / "file").write_text("outside")

    def test_removes_an_exact_file_and_reports_absence(self):
        self.assertTrue(owned_removal.remove_owned(self.root, "owned/file"))
        self.assertFalse((self.root / "owned/file").exists())
        self.assertFalse(owned_removal.remove_owned(self.root, "owned/file"))
        self.assertFalse(owned_removal.remove_owned(self.root, "missing/file"))

    def test_never_removes_through_a_linked_folder_or_linked_root(self):
        (self.root / "linked").symlink_to(self.outside)
        with self.assertRaises(owned_removal.UnsafeRemoval):
            owned_removal.remove_owned(self.root, "linked/file")
        (self.scratch / "linked-root").symlink_to(self.outside)
        with self.assertRaises(owned_removal.UnsafeRemoval):
            owned_removal.remove_owned(self.scratch / "linked-root", "file")
        self.assertEqual((self.outside / "file").read_text(), "outside")

    def test_a_linked_leaf_is_removed_itself_and_tree_links_are_not_followed(self):
        (self.root / "owned/nested/to-outside").symlink_to(self.outside)
        (self.root / "leaf-link").symlink_to(self.outside)
        self.assertTrue(owned_removal.remove_owned(self.root, "leaf-link"))
        self.assertTrue(owned_removal.remove_owned(self.root, "owned"))
        self.assertFalse((self.root / "owned").exists())
        self.assertEqual((self.outside / "file").read_text(), "outside")

    def test_a_replaced_leaf_is_kept(self):
        file = self.root / "owned/file"
        recorded = file.lstat()
        file.rename(self.root / "owned/moved")
        file.write_text("replacement")
        with self.assertRaises(owned_removal.UnsafeRemoval):
            owned_removal.remove_owned(self.root, "owned/file", (recorded.st_dev, recorded.st_ino))
        self.assertEqual(file.read_text(), "replacement")
        current = file.lstat()
        self.assertTrue(owned_removal.remove_owned(self.root, "owned/file", (current.st_dev, current.st_ino)))

    def test_replacement_immediately_before_delete_survives(self):
        record = self.root / "turnserver.pid"
        record.write_text("old")
        info = record.stat()
        expected = info.st_dev, info.st_ino
        unlink = os.unlink
        def replace_then_delete(name, *, dir_fd=None):
            record.write_text("new-owner")
            return unlink(name, dir_fd=dir_fd)
        with mock.patch.object(owned_removal.os, "unlink", replace_then_delete):
            self.assertTrue(owned_removal.remove_owned(self.root, "turnserver.pid", expected))
        self.assertEqual(record.read_text(), "new-owner")

    def test_replacement_between_inspection_and_capture_is_restored(self):
        record = self.root / "turnserver.pid"
        record.write_text("old")
        info = record.stat()
        expected = info.st_dev, info.st_ino
        rename = os.rename
        def replace_then_capture(name, destination, **kwargs):
            rename(record, self.root / "old.pid")
            record.write_text("replacement")
            return rename(name, destination, **kwargs)
        with mock.patch.object(owned_removal.os, "rename", replace_then_capture):
            with self.assertRaises(owned_removal.UnsafeRemoval):
                owned_removal.remove_owned(self.root, "turnserver.pid", expected)
        self.assertEqual(record.read_text(), "replacement")
        self.assertFalse(list(self.root.glob(".elastos-remove-*")))

    def test_restore_conflict_preserves_both_records(self):
        record = self.root / "turnserver.pid"
        record.write_text("old")
        info = record.stat()
        expected = info.st_dev, info.st_ino
        rename = os.rename
        def replace_capture_and_create(name, destination, **kwargs):
            rename(record, self.root / "old.pid")
            record.write_text("captured-replacement")
            rename(name, destination, **kwargs)
            record.write_text("new-owner")
        with mock.patch.object(owned_removal.os, "rename", replace_capture_and_create):
            with self.assertRaises(owned_removal.UnsafeRemoval):
                owned_removal.remove_owned(self.root, "turnserver.pid", expected)
        self.assertEqual(record.read_text(), "new-owner")
        captured = list(self.root.glob(".elastos-remove-*/entry"))
        self.assertEqual(len(captured), 1)
        self.assertEqual(captured[0].read_text(), "captured-replacement")

    def test_linked_ancestor_is_refused(self):
        (self.root / "actual").mkdir()
        record = self.root / "actual/turnserver.pid"
        record.write_text("outside")
        (self.root / "linked").symlink_to(self.root / "actual", target_is_directory=True)
        with self.assertRaises(owned_removal.UnsafeRemoval):
            owned_removal.remove_owned(self.root / "linked", "turnserver.pid")
        (self.root / "actual/nested").mkdir()
        (self.root / "actual/nested/turnserver.pid").write_text("outside")
        with self.assertRaises(owned_removal.UnsafeRemoval):
            owned_removal.remove_owned(self.root / "linked/nested", "turnserver.pid")
        self.assertEqual(record.read_text(), "outside")

    def test_only_plain_relative_names_are_accepted(self):
        for relative in ("", "/etc/passwd", "../outside/file", "owned/../owned/file", "./owned/file",
                         "owned//file"):
            with self.subTest(relative=relative), self.assertRaises(owned_removal.UnsafeRemoval):
                owned_removal.remove_owned(self.root, relative)
        self.assertTrue((self.root / "owned/file").exists())

    def test_shell_form_removes_and_refuses_with_exit_codes(self):
        (self.root / "linked").symlink_to(self.outside)
        run = lambda *args: subprocess.run([sys.executable, str(HELPER), *args],
                                           text=True, capture_output=True)
        self.assertEqual(run(str(self.root), "owned/file").returncode, 0)
        self.assertFalse((self.root / "owned/file").exists())
        self.assertEqual(run(str(self.root), "owned/file").returncode, 0)
        refused = run(str(self.root), "linked/file")
        self.assertEqual(refused.returncode, 1)
        self.assertIn("Removal refused", refused.stderr)
        self.assertEqual(run(str(self.root), "owned/nested/file", "--expect", "1:2").returncode, 1)
        self.assertTrue((self.root / "owned/nested/file").exists())
        self.assertEqual(run(str(self.root)).returncode, 2)


if __name__ == "__main__":
    unittest.main()
