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
                         "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "ELASTOS_RELEASE_VERSION",
                         "DEP_FLAG", "PLAIN_NOTE", "APP_NOTE"}
    environment = {key: value for key, value in os.environ.items()
                   if key not in skip and not key.startswith("CARGO_PROFILE_")}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"})
    return environment


def package(name, extra=""):
    return textwrap.dedent("""\
        [package]
        name = "%s"
        version = "0.1.0"
        edition = "2021"
        """ % name) + extra


FILES = {
    ".gitignore": "target/\n",
    "ws/Cargo.toml": '[workspace]\nmembers = ["app", "other", "macro-user"]\nresolver = "2"\n',
    "ws/app/Cargo.toml": package("app", textwrap.dedent("""\
        [dependencies]
        dep = { path = "../../dep" }
        plain = { path = "../../plain" }
        gitdep = { git = "GITDEP_URL", rev = "GITDEP_REV" }
        cfg-if = "1"
        """)),
    "ws/app/src/main.rs": textwrap.dedent("""\
        const ASSET: &[u8] = include_bytes!("../../../assets/a.txt");
        #[cfg(test)]
        const TEST_ONLY: &str = include_str!("../../../assets/test-only.txt");
        fn main() {
            println!("{} {} {} {} {:?}", ASSET.len(), dep::f(), plain::g(), gitdep::h(), option_env!("APP_NOTE"));
        }
        #[cfg(test)] mod t { #[test] fn ok() { assert!(!super::TEST_ONLY.is_empty()); } }
        """),
    "ws/other/Cargo.toml": package("other", '[dependencies]\neither = "1"\n'),
    "ws/other/src/lib.rs": "pub fn o() -> either::Either<u8, u8> { either::Either::Left(0) }\n",
    "ws/macro-user/Cargo.toml": package("macro-user", '[dependencies]\npm = { path = "../../pm" }\n'),
    "ws/macro-user/src/main.rs": "fn main() { println!(\"{}\", pm::embed!()); }\n",
    "pm/Cargo.toml": package("pm", "[lib]\nproc-macro = true\n"),
    "pm/src/lib.rs": textwrap.dedent("""\
        use proc_macro::TokenStream;
        #[proc_macro]
        pub fn embed(_input: TokenStream) -> TokenStream {
            let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/pm.txt")).unwrap();
            format!("{:?}", text).parse().unwrap()
        }
        """),
    "dep/Cargo.toml": package("dep"),
    "dep/build.rs": textwrap.dedent("""\
        fn main() {
            println!("cargo:rerun-if-env-changed=DEP_FLAG");
            println!("cargo:rerun-if-changed=build.rs");
        }
        """),
    "dep/src/lib.rs": "pub fn f() -> u32 { 1 }\n",
    "plain/Cargo.toml": package("plain"),
    # Reads PLAIN_NOTE without declaring it and generates an included file.
    "plain/build.rs": textwrap.dedent("""\
        use std::{env, fs, path::Path};
        fn main() {
            let note = env::var("PLAIN_NOTE").unwrap_or_default();
            let out = Path::new(&env::var("OUT_DIR").unwrap()).join("gen.rs");
            fs::write(out, format!("pub const NOTE: &str = {:?};\\n", note)).unwrap();
        }
        """),
    "plain/src/lib.rs": 'include!(concat!(env!("OUT_DIR"), "/gen.rs"));\npub fn g() -> u32 { NOTE.len() as u32 + 2 }\n',
    "assets/a.txt": "hello\n",
    "assets/test-only.txt": "tests only\n",
    "assets/pm.txt": "macro input\n",
    "scripts/build-key-edges.json": json.dumps(
        {"packages": {"app": ["assets/a.txt", "assets/test-only.txt"]}, "units": {}}),
}
GITDEP = {"Cargo.toml": package("gitdep"), "src/lib.rs": "pub fn h() -> u32 { 4 }\n"}


class BuildKeyTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory()
        scratch = Path(os.path.realpath(cls.scratch.name))
        cls.root = scratch / "fixture"
        cls.env = fixture_environment()
        cls.env["CARGO_HOME"] = str(scratch / "cargo-home")
        cls.env["CARGO_TARGET_DIR"] = str(cls.root / "target")
        gitdep = scratch / "gitdep"
        for name, content in GITDEP.items():
            (gitdep / name).parent.mkdir(parents=True, exist_ok=True)
            (gitdep / name).write_text(content)
        cls.git(gitdep, "init", "-q", "-b", "main", ".")
        cls.git(gitdep, "add", "-A")
        cls.git(gitdep, "commit", "-q", "-m", "gitdep")
        rev = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=str(gitdep), env=cls.env, text=True).strip()
        for name, content in FILES.items():
            path = cls.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content.replace("GITDEP_URL", "file://" + str(gitdep)).replace("GITDEP_REV", rev))
        cls.git(cls.root, "init", "-q", "-b", "main", ".")
        cls.git(cls.root, "add", "-A")
        cls.git(cls.root, "commit", "-q", "-m", "fixture")
        cls.edges = cls.root / "scripts" / "build-key-edges.json"
        cls.unit = KEY.Unit(cls.root / "ws/Cargo.toml", "app", "bin")

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    @classmethod
    def git(cls, cwd, *args):
        subprocess.run(["git", *args], cwd=str(cwd), env=cls.env, check=True, stdout=subprocess.DEVNULL)

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

    def add_file(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
        self.addCleanup(path.unlink)

    def test_mtime_only_change_keeps_key(self):
        before, _, _ = self.key()
        time.sleep(0.05)
        for name in FILES:
            os.utime(self.root / name, None)
        after, _, _ = self.key()
        self.assertEqual(before, after)

    def test_dependency_source_byte_changes_key(self):
        before, document, _ = self.key()
        self.assertIn("dep/src/lib.rs", document["trees"]["dep"])
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

    def test_undeclared_build_script_env_read_changes_key(self):
        before, document, _ = self.key()
        self.assertIn("PLAIN_NOTE", document["env"])
        after, _, _ = self.key(env={"PLAIN_NOTE": "changed"})
        self.assertNotEqual(before, after)

    def test_dynamic_build_script_env_read_is_miss(self):
        self.write("plain/build.rs", FILES["plain/build.rs"].replace(
            "let note = env::var(\"PLAIN_NOTE\").unwrap_or_default();",
            "let note = env::vars().map(|(k, _)| k).collect::<Vec<_>>().join(\",\");"))
        key, reason, _ = self.key()
        self.assertIsNone(key)
        self.assertIn("build script of plain reads the environment dynamically", reason)

    def test_proc_macro_reading_files_is_miss(self):
        unit = KEY.Unit(self.root / "ws/Cargo.toml", "macro-user", "bin")
        key, reason, _ = self.key(unit=unit)
        self.assertIsNone(key)
        self.assertIn("proc-macro pm reads the filesystem", reason)

    def test_tampered_generated_input_changes_key(self):
        before, document, _ = self.key()
        self.assertEqual(list(document["generated"]), ["generated:plain/out/gen.rs"])
        generated = list((self.root / "target").glob("debug/build/plain-*/out/gen.rs"))
        self.assertTrue(generated)
        for path in generated:
            original = path.read_text()
            path.write_text(original.replace('"";', '"tampered";'))
            self.addCleanup(path.write_text, original)
        after, _, _ = self.key()
        self.assertNotEqual(before, after)

    def test_tampered_git_checkout_changes_key(self):
        before, document, _ = self.key()
        self.assertEqual(len(document["git"]), 1)
        self.assertTrue(all(name.startswith("git:gitdep-") for name in document["git"]))
        self.assertTrue(any(KEY.parse_package_id(p)[1] == "gitdep" for p in document["lock"]))
        checkout = next(Path(self.env["CARGO_HOME"]).glob("git/checkouts/gitdep-*/*/src/lib.rs"))
        original = checkout.read_text()
        checkout.write_text("pub fn h() -> u32 { 5 }\n")
        self.addCleanup(checkout.write_text, original)
        after, _, _ = self.key()
        self.assertNotEqual(before, after)

    def edit_lock_checksum(self, package, prefix):
        blocks = (self.root / "ws/Cargo.lock").read_text().split("[[package]]")
        edited = [block.replace('checksum = "', 'checksum = "' + prefix) if 'name = "%s"' % package in block
                  else block for block in blocks]
        self.write("ws/Cargo.lock", "[[package]]".join(edited))

    def test_lock_slice_ignores_packages_outside_closure(self):
        before, document, inputs = self.key()
        self.assertEqual(sorted(KEY.parse_package_id(p)[1] for p in document["lock"]), ["cfg-if", "gitdep"])
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
        edges.write_text(json.dumps({"packages": FILES_EDGES["packages"],
                                     "units": {"app/bin": {"paths": ["assets/missing.txt"]}}}))
        key, reason, _ = self.key(edges=edges)
        self.assertIsNone(key)
        self.assertIn("assets/missing.txt is missing", reason)

    def test_unit_edge_directory_is_hashed(self):
        edges = self.root / "scripts" / "dir-edges.json"
        edges.write_text(json.dumps({"packages": FILES_EDGES["packages"],
                                     "units": {"app/bin": {"paths": ["assets"], "env": ["UNIT_VAR"]}}}))
        before, document, _ = self.key(edges=edges)
        self.assertIn("assets/pm.txt", document["files"])
        self.assertIn("UNIT_VAR", document["env"])
        self.add_file("assets/b.txt", "new")
        after, document, _ = self.key(edges=edges)
        self.assertTrue(document["files"]["assets/b.txt"].startswith("sha256:"))
        self.assertNotEqual(before, after)

    def test_new_file_in_package_tree_changes_recorded_key(self):
        key, document, inputs = self.key()
        self.assertEqual(sorted(document["trees"]), ["dep", "plain", "ws/app"])
        self.add_file("ws/app/tests/extra.rs", "#[test] fn extra() {}\n")
        recorded, _, _ = self.key(inputs=inputs)
        self.assertNotEqual(key, recorded)
        fresh, _, _ = self.key()
        self.assertEqual(recorded, fresh)

    def test_edges_map_change_changes_recorded_key(self):
        key, document, inputs = self.key()
        self.assertEqual(document["edges"], KEY.git_blob(self.edges))
        edges = self.root / "scripts" / "more-edges.json"
        edges.write_text(json.dumps({"packages": FILES_EDGES["packages"],
                                     "units": {"app/bin": {"paths": ["assets/pm.txt"]}}}))
        recorded, _, _ = self.key(inputs=inputs, edges=edges)
        self.assertNotEqual(key, recorded)

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

    def test_inputs_are_repository_relative_and_present(self):
        _, document, inputs = self.key()
        names = list(document["files"]) + [n for tree in document["trees"].values() for n in tree]
        for name in names:
            self.assertFalse(name.startswith(("/", "target/", "registry:", "generated:", "git:")), name)
            self.assertTrue((self.root / name).is_file(), name)
        self.assertNotIn("ws/app/src/main.rs", document["files"])
        self.assertIn("ws/app/src/main.rs", document["trees"]["ws/app"])
        self.assertTrue(all(not p.startswith("path+file://") for p in inputs["closure"]))
        self.assertIn("path:ws/app#0.1.0", inputs["closure"])

    def test_test_unit_covers_test_only_includes(self):
        _, bin_document, _ = self.key()
        self.assertNotIn("assets/test-only.txt", bin_document["files"])
        unit = KEY.Unit(self.root / "ws/Cargo.toml", "app", "test")
        key, document, _ = self.key(unit=unit)
        self.assertIsNotNone(key, document)
        self.assertIn("assets/test-only.txt", document["files"])


FILES_EDGES = json.loads(FILES["scripts/build-key-edges.json"])


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
