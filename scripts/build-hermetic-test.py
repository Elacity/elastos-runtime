#!/usr/bin/env python3
"""Hermetic builds of a committed fixture workspace: every known false hit must miss or change the key."""
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest

SCRIPT = Path(__file__).with_name("build-hermetic.py")
SPEC = importlib.util.spec_from_file_location("hermetic", SCRIPT)
HERMETIC = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(HERMETIC)
BK = HERMETIC.BK


def fixture_environment():
    names = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], text=True).splitlines()
    skip = set(names) | {"RUSTFLAGS", "CARGO_ENCODED_RUSTFLAGS", "CARGO_BUILD_RUSTFLAGS", "CARGO_TARGET_DIR",
                         "CARGO_BUILD_TARGET_DIR", "CARGO_BUILD_BUILD_DIR", "ELASTOS_RELEASE_VERSION",
                         "PLAIN_NOTE"}
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
        cfg-if = "1"
        """)),
    "ws/app/src/main.rs": textwrap.dedent("""\
        const ASSET: &[u8] = include_bytes!("../../../assets/a.txt");
        fn main() { println!("{} {} {}", ASSET.len(), dep::f(), plain::note()); }
        """),
    "ws/other/Cargo.toml": package("other", '[dependencies]\neither = "1"\n'),
    "ws/other/src/lib.rs": "pub fn o() -> either::Either<u8, u8> { either::Either::Left(0) }\n",
    "ws/macro-user/Cargo.toml": package("macro-user", '[dependencies]\npm = { path = "../../pm" }\n'),
    "ws/macro-user/src/main.rs": "fn main() { println!(\"{}\", pm::embed!()); }\n",
    "pm/Cargo.toml": package("pm", "[lib]\nproc-macro = true\n"),
    # Round-2 bypass: aliased std::fs read at expansion time.
    "pm/src/lib.rs": textwrap.dedent("""\
        use proc_macro::TokenStream;
        use std::{fs as disk};
        #[proc_macro]
        pub fn embed(_input: TokenStream) -> TokenStream {
            let bytes = disk::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../assets/pm.txt")).unwrap();
            format!("{:?}", String::from_utf8(bytes).unwrap()).parse().unwrap()
        }
        """),
    "dep/Cargo.toml": package("dep"),
    "dep/build.rs": 'fn main() { println!("cargo:rerun-if-env-changed=DEP_FLAG"); }\n',
    "dep/src/lib.rs": "pub fn f() -> u32 { 1 }\n",
    "plain/Cargo.toml": package("plain"),
    # Round-2 bypass: aliased env read, undeclared, smuggled in through rustc-env.
    "plain/build.rs": textwrap.dedent("""\
        use std::env::var as read_env;
        fn main() {
            let note = read_env("PLAIN_NOTE").unwrap_or_else(|_| "zq7-unset".to_string());
            println!("cargo:rustc-env=PLAIN_NOTE_SEEN={}", note);
        }
        """),
    "plain/src/lib.rs": 'pub fn note() -> &\'static str { env!("PLAIN_NOTE_SEEN") }\n',
    "assets/a.txt": "hello\n",
    "assets/pm.txt": "macro input\n",
    "scripts/build-key-edges.json": json.dumps({"packages": {"app": ["assets/a.txt"]}, "units": {}}),
}


class Options:
    def __init__(self, root, **kwargs):
        defaults = {"manifest_path": str(root / "ws/Cargo.toml"), "package": "app", "kind": "bin", "name": None,
                    "target": None, "profile": None, "features": "", "no_default_features": False,
                    "edges": "scripts/build-key-edges.json", "out": None, "work": None, "keep_work": False,
                    "allow_dirty": False, "key_only": False, "diagnose": False, "receipt": None, "json": None}
        defaults.update(kwargs)
        self.__dict__.update(defaults)


class HermeticTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.scratch = tempfile.TemporaryDirectory()
        scratch = Path(os.path.realpath(cls.scratch.name))
        cls.root = scratch / "fixture"
        cls.env = fixture_environment()
        cls.env["CARGO_HOME"] = str(scratch / "cargo-home")
        for name, content in FILES.items():
            path = cls.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        cls.git("init", "-q", "-b", "main", ".")
        subprocess.run(["cargo", "generate-lockfile", "-q"], cwd=str(cls.root / "ws"), env=cls.env, check=True)
        cls.commit("fixture")
        cls.builds = 0

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    @classmethod
    def git(cls, *args):
        subprocess.run(["git", *args], cwd=str(cls.root), env=cls.env, check=True, stdout=subprocess.DEVNULL)

    @classmethod
    def commit(cls, message):
        cls.git("add", "-A")
        cls.git("commit", "-q", "--allow-empty", "-m", message)

    def setUp(self):
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=str(self.root), env=self.env, text=True)
        self.addCleanup(self.git, "reset", "-q", "--hard", head.strip())
        self.addCleanup(self.git, "clean", "-fdq", "-e", "target")

    def run_unit(self, env=None, stable=False, **kwargs):
        """Returns (key, receipt or document) or (None, reason); stable = fixed, wiped paths."""
        type(self).builds += 1
        label = "stable" if stable else "%d" % self.builds
        out = self.root / "target" / ("hermetic-" + label)
        work = Path(self.scratch.name) / ("work-" + label)
        shutil.rmtree(out, ignore_errors=True)
        options = Options(self.root, out=str(out), work=str(work), **kwargs)
        try:
            plan, sandbox, document, key, out, work, commit = HERMETIC.compute(options, dict(self.env, **(env or {})))
            if options.key_only:
                return key, document
            outputs, seconds = HERMETIC.build(plan, sandbox, out)
            if options.diagnose:
                HERMETIC.diagnose(plan, out, plan.environment(out, sandbox.home, sandbox.cargo_home,
                                                               sandbox.rustup_home))
            return key, {"document": document, "outputs": outputs}
        except BK.Miss as miss:
            return None, str(miss)
        finally:
            shutil.rmtree(work, ignore_errors=True)

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def executable(self, receipt):
        digests = [d for path, d in receipt["outputs"].items() if path.endswith("/app")]
        self.assertEqual(len(digests), 1, receipt["outputs"])
        return digests[0]

    def built_app(self):
        return (self.root / "target" / "hermetic-stable" / "debug" / "app").read_bytes()

    def test_same_commit_builds_identical_outputs_and_key(self):
        # Byte-identical outputs need the same checkout and target paths
        # (debug info embeds them); producers use stable --work and --out.
        first, receipt = self.run_unit(diagnose=True, stable=True)
        self.assertIsNotNone(first, receipt)
        second, again = self.run_unit(stable=True)
        self.assertEqual(first, second)
        if sys.platform != "darwin":  # ld64 stamps object mtimes into debug binaries
            self.assertEqual(self.executable(receipt), self.executable(again))
        self.assertNotIn("generated", receipt["document"])
        self.assertEqual(sorted(receipt["document"]["trees"]),
                         ["dep", "plain", "pm", "ws/app", "ws/macro-user", "ws/other"])

    def test_key_only_equals_built_key(self):
        computed, _ = self.run_unit(key_only=True)
        built, receipt = self.run_unit()
        self.assertEqual(computed, built, receipt)

    def test_environment_outside_allowlist_cannot_reach_the_build(self):
        key_a, receipt_a = self.run_unit(env={"PLAIN_NOTE": "zq7-alpha-marker"}, stable=True)
        self.assertIsNotNone(key_a, receipt_a)
        binary_a = self.built_app()
        key_b, receipt_b = self.run_unit(env={"PLAIN_NOTE": "zq7-beta-marker"}, stable=True)
        binary_b = self.built_app()
        self.assertEqual(key_a, key_b)
        self.assertNotIn("PLAIN_NOTE", receipt_a["document"]["env"])
        for binary in (binary_a, binary_b):
            self.assertIn(b"zq7-unset", binary)
            self.assertNotIn(b"zq7-alpha", binary)
            self.assertNotIn(b"zq7-beta", binary)
        if sys.platform != "darwin":
            self.assertEqual(binary_a, binary_b)

    def test_allowlisted_environment_changes_key(self):
        before, _ = self.run_unit(key_only=True)
        versioned, document = self.run_unit(key_only=True, env={"ELASTOS_RELEASE_VERSION": "1.2.3"})
        self.assertEqual(document["env"]["ELASTOS_RELEASE_VERSION"], "1.2.3")
        self.assertNotEqual(before, versioned)
        flags, _ = self.run_unit(key_only=True, env={"RUSTFLAGS": "-C opt-level=1"})
        self.assertNotEqual(before, flags)

    def test_proc_macro_undeclared_file_read_is_miss(self):
        key, reason = self.run_unit(package="macro-user")
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)
        self.write("scripts/build-key-edges.json", json.dumps(
            {"packages": {"app": ["assets/a.txt"], "pm": ["assets/pm.txt"]}, "units": {}}))
        self.commit("declare pm edge")
        declared, receipt = self.run_unit(package="macro-user")
        self.assertIsNotNone(declared, receipt)
        self.assertIn("assets/pm.txt", receipt["document"]["files"])
        self.write("assets/pm.txt", "macro input changed\n")
        self.commit("change pm input")
        changed, _ = self.run_unit(package="macro-user", key_only=True)
        self.assertNotEqual(declared, changed)

    def test_ignored_generated_module_is_miss(self):
        self.write(".gitignore", "target/\nws/app/src/generated.rs\n")
        self.write("ws/app/src/generated.rs", "pub const G: u32 = 7;\n")
        self.write("ws/app/src/main.rs", "mod generated;\n" + FILES["ws/app/src/main.rs"].replace(
            "dep::f()", "dep::f() + generated::G"))
        self.commit("use ignored generated module")
        plain = subprocess.run(["cargo", "build", "-q", "-p", "app"], cwd=str(self.root / "ws"),
                               env=dict(self.env, CARGO_TARGET_DIR=str(self.root / "target" / "plain")),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.assertEqual(plain.returncode, 0, "the worktree build must succeed to show the trap")
        key, reason = self.run_unit()
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_undeclared_cross_package_include_is_miss(self):
        self.write("scripts/build-key-edges.json", json.dumps({"packages": {}, "units": {}}))
        self.commit("drop declared edge")
        key, reason = self.run_unit()
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_absolute_read_outside_sandbox_is_miss(self):
        outside = Path(self.scratch.name) / "outside.txt"
        outside.write_text("x")
        self.write("plain/build.rs", FILES["plain/build.rs"].replace(
            'let note = read_env("PLAIN_NOTE").unwrap_or_else(|_| "zq7-unset".to_string());',
            'let note = std::fs::read_to_string(%s).unwrap();' % json.dumps(str(outside))))
        self.commit("read outside")
        key, reason = self.run_unit()
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_new_integration_test_changes_key(self):
        before, _ = self.run_unit(key_only=True)
        self.write("ws/app/tests/extra.rs", "#[test] fn extra() {}\n")
        self.commit("add integration test")
        after, _ = self.run_unit(key_only=True)
        self.assertNotEqual(before, after)

    def test_dependency_source_byte_changes_key(self):
        before, _ = self.run_unit(key_only=True)
        self.write("dep/src/lib.rs", "pub fn f() -> u32 { 2 }\n")
        self.commit("change dep")
        after, _ = self.run_unit(key_only=True)
        self.assertNotEqual(before, after)

    def test_edges_map_change_changes_key(self):
        before, document = self.run_unit(key_only=True)
        self.assertEqual(document["edges"], BK.git_blob(self.root / "scripts/build-key-edges.json"))
        self.write("scripts/build-key-edges.json", json.dumps(
            {"packages": {"app": ["assets/a.txt"]}, "units": {"app/bin": {"paths": ["assets/pm.txt"]}}}))
        self.commit("add unit edge")
        after, document = self.run_unit(key_only=True)
        self.assertNotEqual(before, after)
        self.assertIn("assets/pm.txt", document["files"])

    def test_lock_slice_covers_closure_only_and_corrupt_lock_is_miss(self):
        _, document = self.run_unit(key_only=True)
        self.assertEqual([BK.parse_package_id(p)[1] for p in document["lock"]], ["cfg-if"])
        self.assertNotIn("path:ws/other#0.1.0", document["closure"])
        lock = self.root / "ws/Cargo.lock"
        blocks = lock.read_text().split("[[package]]")
        either = next(b for b in blocks if 'name = "either"' in b)
        checksum = either.split('checksum = "')[1].split('"')[0]
        self.assertNotIn(checksum, BK.canonical(document))
        lock.write_text("[[package]]".join(b.replace(checksum, "0000" + checksum[4:]) for b in blocks))
        self.commit("corrupt unrelated lock entry")
        key, reason = self.run_unit(key_only=True)
        self.assertIsNone(key)
        self.assertIn("checksum for `either", reason)

    def test_dirty_worktree_is_refused(self):
        self.write("assets/a.txt", "dirty\n")
        key, reason = self.run_unit(key_only=True)
        self.assertIsNone(key)
        self.assertIn("uncommitted changes", reason)

    def test_test_unit_includes_dev_dependencies(self):
        self.write("ws/app/Cargo.toml", FILES["ws/app/Cargo.toml"] + '[dev-dependencies]\nother = { path = "../other" }\n')
        subprocess.run(["cargo", "generate-lockfile", "-q"], cwd=str(self.root / "ws"), env=self.env, check=True)
        self.commit("dev dependency")
        _, bin_document = self.run_unit(key_only=True)
        _, test_document = self.run_unit(key_only=True, kind="test")
        self.assertNotIn("path:ws/other#0.1.0", bin_document["closure"])
        self.assertIn("path:ws/other#0.1.0", test_document["closure"])
        self.assertIn("either", [BK.parse_package_id(p)[1] for p in test_document["lock"]])


if __name__ == "__main__":
    if shutil.which("cargo") is None:
        print("cargo not found; hermetic tests need a Rust toolchain", file=sys.stderr)
        sys.exit(1)
    if shutil.which("bwrap" if sys.platform != "darwin" else "sandbox-exec") is None:
        print("no sandbox available (bwrap on Linux, sandbox-exec on macOS)", file=sys.stderr)
        sys.exit(1)
    unittest.main()
