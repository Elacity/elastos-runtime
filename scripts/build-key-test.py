#!/usr/bin/env python3
"""Exercise build-key decisions on a small fixture workspace built with the real cargo."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest

SCRIPT = Path(__file__).with_name("build-key.py")
SPEC = importlib.util.spec_from_file_location("buildkey", SCRIPT)
KEY = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(KEY)
REPO = Path(__file__).resolve().parent.parent


def fixture_environment():
    names = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], text=True).splitlines()
    skip = set(names) | {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_DIR",
                         "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "ELASTOS_RELEASE_VERSION", "DEP_FLAG"}
    environment = {key: value for key, value in os.environ.items()
                   if key not in skip and not key.startswith("CARGO_PROFILE_")}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"})
    return environment


FILES = {
    ".gitignore": "target/\n",
    "ws/Cargo.toml": textwrap.dedent("""\
        [workspace]
        members = ["app", "other"]
        resolver = "2"
        """),
    "ws/app/Cargo.toml": textwrap.dedent("""\
        [package]
        name = "app"
        version = "0.1.0"
        edition = "2021"
        [dependencies]
        dep = { path = "../../dep" }
        plain = { path = "../../plain" }
        cfg-if = "1"
        """),
    "ws/app/src/main.rs": textwrap.dedent("""\
        const ASSET: &[u8] = include_bytes!("../../../assets/a.txt");
        fn main() { println!("{} {} {} {:?}", ASSET.len(), dep::f(), plain::g(), option_env!("APP_NOTE")); }
        #[cfg(test)] mod t { #[test] fn ok() {} }
        """),
    "ws/other/Cargo.toml": textwrap.dedent("""\
        [package]
        name = "other"
        version = "0.1.0"
        edition = "2021"
        [dependencies]
        either = "1"
        """),
    "ws/other/src/lib.rs": "pub fn o() -> either::Either<u8, u8> { either::Either::Left(0) }\n",
    "dep/Cargo.toml": textwrap.dedent("""\
        [package]
        name = "dep"
        version = "0.1.0"
        edition = "2021"
        """),
    "dep/build.rs": textwrap.dedent("""\
        fn main() {
            println!("cargo:rerun-if-env-changed=DEP_FLAG");
            println!("cargo:rerun-if-changed=build.rs");
        }
        """),
    "dep/src/lib.rs": "pub fn f() -> u32 { 1 }\n",
    "plain/Cargo.toml": textwrap.dedent("""\
        [package]
        name = "plain"
        version = "0.1.0"
        edition = "2021"
        """),
    "plain/build.rs": "fn main() {}\n",
    "plain/src/lib.rs": "pub fn g() -> u32 { 2 }\n",
    "assets/a.txt": "hello\n",
    "scripts/build-key-edges.json": json.dumps({"packages": {"app": ["assets/a.txt"]}, "units": {}}),
}


class BuildKeyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory()
        cls.root = Path(os.path.realpath(cls.scratch.name)) / "fixture"
        for name, content in FILES.items():
            path = cls.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        cls.env = fixture_environment()
        cls.env["CARGO_TARGET_DIR"] = str(cls.root / "target")
        cls.git("init", "-q", "-b", "main", ".")
        cls.git("add", "-A")
        cls.git("commit", "-q", "-m", "fixture")
        cls.edges = cls.root / "scripts" / "build-key-edges.json"
        cls.unit = KEY.Unit(cls.root / "ws/Cargo.toml", "app", "bin")

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    @classmethod
    def git(cls, *args):
        subprocess.run(["git", *args], cwd=str(cls.root), env=cls.env, check=True, stdout=subprocess.DEVNULL)

    def key(self, env=None, edges=None, inputs=None, unit=None):
        environ = dict(self.env, **(env or {}))
        try:
            key, document, inputs = KEY.compute_key(unit or self.unit, self.root, environ,
                                                    edges or self.edges, inputs)
        except KEY.Miss as miss:
            return None, str(miss), None
        return key, document, inputs

    def write(self, name, content):
        path = self.root / name
        original = path.read_text()
        path.write_text(content)
        self.addCleanup(path.write_text, original)

    def test_mtime_only_change_keeps_key(self):
        before, _, _ = self.key()
        later = time.time() + 120
        for name in FILES:
            os.utime(self.root / name, (later, later))
        after, _, _ = self.key()
        self.assertEqual(before, after)

    def test_dependency_source_byte_changes_key(self):
        before, document, _ = self.key()
        self.assertIn("dep/src/lib.rs", document["files"])
        self.write("dep/src/lib.rs", "pub fn f() -> u32 { 2 }\n")
        after, _, _ = self.key()
        self.assertNotEqual(before, after)

    def test_include_bytes_outside_crate_dir_changes_key(self):
        before, document, _ = self.key()
        self.assertIn("assets/a.txt", document["files"])
        self.write("assets/a.txt", "hello!\n")
        after, _, _ = self.key()
        self.assertNotEqual(before, after)

    def test_rerun_if_env_changed_var_changes_key(self):
        before, document, _ = self.key()
        self.assertIsNone(document["env"]["DEP_FLAG"])
        after, document, _ = self.key(env={"DEP_FLAG": "1"})
        self.assertEqual(document["env"]["DEP_FLAG"], "1")
        self.assertNotEqual(before, after)

    def test_option_env_read_changes_key(self):
        before, document, _ = self.key()
        self.assertIn("APP_NOTE", document["env"])
        after, _, _ = self.key(env={"APP_NOTE": "x"})
        self.assertNotEqual(before, after)

    def edit_lock_checksum(self, package, prefix):
        blocks = (self.root / "ws/Cargo.lock").read_text().split("[[package]]")
        edited = [block.replace('checksum = "', 'checksum = "' + prefix) if 'name = "%s"' % package in block
                  else block for block in blocks]
        self.write("ws/Cargo.lock", "[[package]]".join(edited))

    def test_lock_slice_ignores_packages_outside_closure(self):
        before, document, inputs = self.key()
        self.assertEqual([KEY.parse_package_id(p)[1] for p in document["lock"]], ["cfg-if"])
        self.edit_lock_checksum("either", "0000")
        same, _, _ = self.key(inputs=inputs)
        self.assertEqual(before, same)
        self.edit_lock_checksum("cfg-if", "ffff")
        changed, _, _ = self.key(inputs=inputs)
        self.assertNotEqual(before, changed)

    def test_rustflags_change_changes_key(self):
        before, _, _ = self.key()
        after, document, _ = self.key(env={"RUSTFLAGS": "-C opt-level=1"})
        self.assertEqual(document["build_env"]["RUSTFLAGS"], "-C opt-level=1")
        self.assertNotEqual(before, after)

    def test_release_version_and_profile_change_key(self):
        before, _, _ = self.key()
        versioned, _, _ = self.key(env={"ELASTOS_RELEASE_VERSION": "1.2.3"})
        self.assertNotEqual(before, versioned)
        release, _, _ = self.key(unit=KEY.Unit(self.root / "ws/Cargo.toml", "app", "bin", profile="release"))
        self.assertNotEqual(before, release)

    def test_undeclared_absolute_input_is_miss(self):
        outside = Path(self.scratch.name) / "outside.txt"
        outside.write_text("x")
        self.write("ws/app/src/main.rs", FILES["ws/app/src/main.rs"].replace(
            '"../../../assets/a.txt"', json.dumps(str(outside))))
        key, reason, _ = self.key()
        self.assertIsNone(key)
        self.assertIn("undeclared absolute input " + str(outside), reason)

    def test_undeclared_cross_package_input_is_miss(self):
        bare = self.root / "scripts" / "bare-edges.json"
        bare.write_text(json.dumps({"packages": {}, "units": {}}))
        key, reason, _ = self.key(edges=bare)
        self.assertIsNone(key)
        self.assertIn("assets/a.txt (read by app)", reason)

    def test_missing_declared_unit_edge_is_miss(self):
        edges = self.root / "scripts" / "unit-edges.json"
        edges.write_text(json.dumps({"packages": {"app": ["assets/a.txt"]},
                                     "units": {"app/bin": {"paths": ["assets/missing.txt"]}}}))
        key, reason, _ = self.key(edges=edges)
        self.assertIsNone(key)
        self.assertIn("assets/missing.txt is missing", reason)

    def test_unit_edge_directory_is_hashed(self):
        edges = self.root / "scripts" / "dir-edges.json"
        edges.write_text(json.dumps({"packages": {"app": ["assets/a.txt"]},
                                     "units": {"app/bin": {"paths": ["assets"], "env": ["UNIT_VAR"]}}}))
        before, document, _ = self.key(edges=edges)
        self.assertIn("assets/a.txt", document["files"])
        self.assertIn("UNIT_VAR", document["env"])
        extra = self.root / "assets" / "b.txt"
        extra.write_text("new")
        self.addCleanup(extra.unlink)
        after, document, _ = self.key(edges=edges)
        self.assertTrue(document["files"]["assets/b.txt"].startswith("sha256:"))
        self.assertNotEqual(before, after)

    def test_build_script_without_declarations_hashes_package_tree(self):
        before, document, inputs = self.key()
        self.assertEqual(inputs["trees"], ["plain"])
        self.assertIn("plain/build.rs", document["files"])
        extra = self.root / "plain" / "notes.txt"
        extra.write_text("x")
        self.addCleanup(extra.unlink)
        after, _, _ = self.key()
        self.assertNotEqual(before, after)

    def test_recorded_inputs_reproduce_key_without_cargo(self):
        key, _, inputs = self.key()
        recorded = json.loads(json.dumps(inputs))
        same, _, _ = self.key(inputs=recorded)
        self.assertEqual(key, same)
        self.write("dep/src/lib.rs", "pub fn f() -> u32 { 3 }\n")
        changed, _, _ = self.key(inputs=recorded)
        self.assertNotEqual(key, changed)
        other = KEY.Unit(self.root / "ws/Cargo.toml", "app", "test")
        key, reason, _ = self.key(inputs=recorded, unit=other)
        self.assertIsNone(key)
        self.assertIn("does not describe this unit", reason)

    def test_generated_and_registry_files_are_not_hashed(self):
        _, document, inputs = self.key()
        self.assertFalse(any(name.startswith("target/") or name.startswith("registry:") for name in document["files"]))
        self.assertEqual(sorted(document["files"]), [
            "assets/a.txt", "dep/Cargo.toml", "dep/build.rs", "dep/src/lib.rs", "plain/Cargo.toml",
            "plain/build.rs", "plain/src/lib.rs", "ws/Cargo.toml", "ws/app/Cargo.toml", "ws/app/src/main.rs"])
        self.assertEqual(sorted(inputs["closure"]), [
            "path:dep#0.1.0", "path:plain#0.1.0", "path:ws/app#0.1.0",
            "registry+https://github.com/rust-lang/crates.io-index#cfg-if@" + next(
                KEY.parse_package_id(p)[2] for p in document["lock"])])

    def test_test_unit_covers_test_only_includes(self):
        unit = KEY.Unit(self.root / "ws/Cargo.toml", "app", "test")
        key, document, _ = self.key(unit=unit)
        self.assertIsNotNone(key, document)
        self.assertIn("ws/app/src/main.rs", document["files"])


class EdgesMapTests(unittest.TestCase):
    def test_declared_edges_exist_in_repository(self):
        edges = json.loads((REPO / "scripts" / "build-key-edges.json").read_text())
        paths = [p for entries in edges["packages"].values() for p in entries]
        paths += [p for unit in edges["units"].values() for p in unit.get("paths", [])]
        missing = [p for p in paths if not (REPO / p).exists()]
        self.assertEqual(missing, [])


if __name__ == "__main__":
    if shutil.which("cargo") is None:
        print("cargo not found; build-key tests need a Rust toolchain", file=sys.stderr)
        sys.exit(1)
    unittest.main()
