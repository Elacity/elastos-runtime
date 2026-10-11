#!/usr/bin/env python3
"""Fixtures for scripts/removal-guard.py and the Python/shell removal helper."""
import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest
from unittest import mock

sys.dont_write_bytecode = True
SOURCE = Path(__file__).resolve().parent
GUARD = SOURCE / "removal-guard.py"
HELPER = SOURCE / "lib/owned_removal.py"
spec = importlib.util.spec_from_file_location("owned_removal", HELPER)
owned_removal = importlib.util.module_from_spec(spec)
spec.loader.exec_module(owned_removal)


def fixture_environment(cwd):
    names = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], cwd=cwd, text=True).splitlines()
    environment = {key: value for key, value in os.environ.items() if key not in names}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"})
    return environment


class RemovalGuardTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory()
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.env = fixture_environment(self.root)
        self.git("init", "-q", "-b", "base")
        # Existing raw calls are not the gate's business: only added lines are.
        self.write("scripts/existing.py", "import os\nos.unlink('/tmp/x')\n")
        self.write("elastos/crates/demo/src/lib.rs", "mod tests;\nmod store;\n")
        self.write("elastos/crates/demo/src/store.rs", "pub fn keep() {}\n")
        self.commit("base")
        self.git("checkout", "-q", "-b", "feature")

    def git(self, *args):
        return subprocess.run(["git", *args], cwd=self.root, env=self.env, check=True,
                              text=True, capture_output=True).stdout

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(text))

    def commit(self, message):
        self.git("add", "-A")
        self.git("commit", "-q", "-m", message)

    def guard(self, *args):
        return subprocess.run([sys.executable, str(GUARD), "--base", "base", *args], cwd=self.root,
                              env=self.env, text=True, capture_output=True)

    def change(self, relative, text):
        self.write(relative, text)
        self.commit(relative)
        return self.guard()

    def assert_refused(self, result, *locations):
        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        for location in locations:
            self.assertIn(f"{location}: raw deletion added to product code", result.stdout)
        self.assertEqual(result.stdout.count("raw deletion added"), len(locations), result.stdout)
        self.assertIn("fix: call ", result.stdout)

    def assert_passed(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_raw_unlink_added_to_product_code_is_refused_with_location_and_fix(self):
        result = self.change("scripts/tool.py", """\
            import os
            from pathlib import Path
            os.unlink(target)
            Path(target).unlink(missing_ok=True)
            """)
        self.assert_refused(result, "scripts/tool.py:3", "scripts/tool.py:4")
        self.assertIn("scripts/lib/owned_removal.py", result.stdout)
        self.assertNotIn("existing.py", result.stdout)

    def test_every_language_form_is_refused(self):
        self.write("elastos/crates/demo/src/store.rs", """\
            pub fn keep() {}
            pub fn drop_it(path: &std::path::Path) {
                std::fs::remove_dir_all(path).unwrap();
                let _ = tokio::fs::remove_file(path);
            }
            """)
        self.write("scripts/tool.mjs", "fs.rmSync(dir, { recursive: true });\nawait fsp.unlink(file);\n")
        self.write("scripts/tool.sh", 'rm -rf "${DATA_DIR}/cache"\nfind "$dir" -name x -delete\n')
        self.write("scripts/spawn.py", 'subprocess.run(["rm", "-f", path])\nshutil.rmtree(path)\n')
        self.commit("raw")
        self.assert_refused(self.guard(), "elastos/crates/demo/src/store.rs:3",
                            "elastos/crates/demo/src/store.rs:4", "scripts/tool.mjs:1", "scripts/tool.mjs:2",
                            "scripts/tool.sh:1", "scripts/tool.sh:2", "scripts/spawn.py:1", "scripts/spawn.py:2")

    def test_python_embedded_in_shell_is_refused(self):
        # The source-home TURN retirement shape: a heredoc unlinking a PID file.
        result = self.change("scripts/setup.sh", """\
            retire() {
                python3 - <<'PY'
            pid_file = root / 'runtime-turn/turnserver.pid'
            pid_file.unlink()
            PY
            }
            """)
        self.assert_refused(result, "scripts/setup.sh:4")

    def test_marker_with_a_reason_passes_and_an_empty_reason_does_not(self):
        self.assert_passed(self.change("scripts/tool.py", """\
            os.unlink(stage)  # removal-ok: mkstemp file this process created and holds open
            # removal-ok: temp dir from mkdtemp above, removed by its exact path
            shutil.rmtree(scratch)
            """))
        self.assert_passed(self.change("scripts/stage.sh", """\
            trap 'rm -rf "${stage}"' EXIT  # removal-ok: mktemp -d directory this function created
            """))
        self.assert_refused(self.change("scripts/other.py", "os.unlink(x)  # removal-ok: ok\n"),
                            "scripts/other.py:1")

    def test_helper_calls_pass(self):
        self.write("scripts/tool.py", "removed = remove_owned(data_dir, 'runtime-turn/turnserver.pid', identity)\n")
        self.write("scripts/tool.sh", 'python3 "${ROOT}/scripts/lib/owned_removal.py" "$DATA_DIR" runtime-turn/turnserver.pid\n')
        self.write("scripts/tool.mjs", 'removeOwned(dataDir, "runtime-turn/turnserver.pid", recorded);\n')
        self.write("elastos/crates/demo/src/store.rs", """\
            pub fn keep() {}
            pub fn retire(data: &Path) {
                elastos_common::owned_removal::remove_owned(data, Path::new("bin/old"), None).unwrap();
            }
            """)
        self.commit("helpers")
        self.assert_passed(self.guard())

    def test_tests_fixtures_and_rust_test_code_are_ignored(self):
        raw = "os.unlink(path)\n"
        for name in ("scripts/tool-test.py", "scripts/tool.test.py", "scripts/tool.test.mjs",
                     "scripts/tool-smoke.sh", "scripts/fixtures/tool.py", "elastos/crates/demo/tests/it.rs"):
            self.write(name, raw if not name.endswith((".mjs", ".sh")) else "rm -f \"$x\"\nfs.unlinkSync(x);\n")
        self.write("elastos/crates/demo/src/tests.rs", "fn f() { std::fs::remove_file(p).unwrap(); }\n")
        self.write("elastos/crates/demo/src/store.rs", """\
            pub fn keep() {}

            #[cfg(test)]
            mod tests {
                #[test]
                fn removes() {
                    std::fs::remove_file(path).unwrap();
                }
            }
            """)
        self.commit("tests")
        self.assert_passed(self.guard())

    def test_rust_code_after_a_test_module_is_still_checked(self):
        result = self.change("elastos/crates/demo/src/store.rs", """\
            pub fn keep() {}

            #[cfg(test)]
            mod tests {
                fn helper() {
                    std::fs::remove_file(path).unwrap();
                }
            }

            pub fn later(path: &Path) {
                std::fs::remove_file(path).unwrap();
            }
            """)
        self.assert_refused(result, "elastos/crates/demo/src/store.rs:11")

    def test_missing_base_is_refused_not_passed(self):
        result = subprocess.run([sys.executable, str(GUARD), "--base", "origin/missing"], cwd=self.root,
                                env=self.env, text=True, capture_output=True)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertIn("unavailable", result.stderr)


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
