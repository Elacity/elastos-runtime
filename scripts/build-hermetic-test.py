#!/usr/bin/env python3
"""Hermetic builds of a committed fixture workspace: every known false hit must miss or change the key.

Linux needs bwrap and HERMETIC_SYSROOT=<sealed rootfs> (see build-hermetic.py make-sysroot);
macOS needs sandbox-exec.
"""
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
LINUX = sys.platform != "darwin"
SYSROOT = os.environ.get("HERMETIC_SYSROOT")


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
    "ws/Cargo.toml": '[workspace]\nmembers = ["app", "other", "macro-user", "ticking"]\nresolver = "2"\n',
    "ws/app/Cargo.toml": package("app", textwrap.dedent("""\
        [dependencies]
        dep = { path = "../../dep" }
        plain = { path = "../../plain" }
        gitdep = { git = "GITDEP_URL", rev = "GITDEP_REV" }
        cfg-if = "1"
        """)),
    "ws/app/src/main.rs": textwrap.dedent("""\
        const ASSET: &[u8] = include_bytes!("../../../assets/a.txt");
        fn main() { println!("{} {} {} {}", ASSET.len(), dep::f(), plain::note(), gitdep::h()); }
        """),
    "ws/other/Cargo.toml": package("other", '[dependencies]\neither = "1"\n'),
    "ws/other/src/lib.rs": "pub fn o() -> either::Either<u8, u8> { either::Either::Left(0) }\n",
    "ws/macro-user/Cargo.toml": package("macro-user", '[dependencies]\npm = { path = "../../pm" }\n'),
    "ws/macro-user/src/main.rs": "fn main() { println!(\"{}\", pm::embed!()); }\n",
    # A build script that stamps the clock into the binary: never reusable.
    "ws/ticking/Cargo.toml": package("ticking", "build = \"build.rs\"\n"),
    "ws/ticking/build.rs": textwrap.dedent("""\
        fn main() {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap();
            println!("cargo:rustc-env=BUILD_STAMP=zq7-{}", now.as_nanos());
        }
        """),
    "ws/ticking/src/main.rs": 'fn main() { println!("{}", env!("BUILD_STAMP")); }\n',
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
GITDEP = {"Cargo.toml": package("gitdep"), "src/lib.rs": "pub fn h() -> u32 { 4 }\n"}


class Options:
    def __init__(self, root, **kwargs):
        defaults = {"manifest_path": str(root / "ws/Cargo.toml"), "package": "app", "kind": "bin", "name": None,
                    "target": None, "profile": None, "features": "", "no_default_features": False,
                    "edges": "scripts/build-key-edges.json", "sysroot": SYSROOT, "out": None, "work": None,
                    "keep_work": False, "allow_dirty": False, "key_only": False, "single": False,
                    "diagnose": False, "receipt": None, "json": None}
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
        subprocess.run(["cargo", "generate-lockfile", "-q"], cwd=str(cls.root / "ws"), env=cls.env, check=True)
        cls.commit("fixture")
        cls.runs = 0

    @classmethod
    def tearDownClass(cls):
        cls.scratch.cleanup()

    @classmethod
    def git(cls, cwd, *args):
        subprocess.run(["git", *args], cwd=str(cwd), env=cls.env, check=True, stdout=subprocess.DEVNULL)

    @classmethod
    def commit(cls, message):
        cls.git(cls.root, "add", "-A")
        cls.git(cls.root, "commit", "-q", "--allow-empty", "-m", message)

    def setUp(self):
        head = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=str(self.root), env=self.env, text=True)
        self.addCleanup(self.git, self.root, "reset", "-q", "--hard", head.strip())
        self.addCleanup(self.git, self.root, "clean", "-fdq", "-e", "target")

    def options(self, **kwargs):
        """Linux: fresh host paths per run (the sandbox canonicalises them); macOS: fixed paths, wiped."""
        type(self).runs += 1
        if LINUX:
            out, work = self.root / "target" / ("run-%d" % self.runs), None
        else:
            out, work = self.root / "target" / "hermetic", str(HERMETIC.DARWIN_WORK)
            shutil.rmtree(out, ignore_errors=True)
        return Options(self.root, out=str(out), work=kwargs.pop("work", work), **kwargs)

    def produce(self, env=None, **kwargs):
        """(key, receipt) or (None, reason)."""
        options = self.options(**kwargs)
        producer = None
        try:
            receipt, producer = HERMETIC.produce(options, dict(self.env, **(env or {})))
            return receipt["key"], receipt
        except BK.Miss as miss:
            return None, str(miss)
        finally:
            if producer is not None and producer.temporary:
                shutil.rmtree(producer.work, ignore_errors=True)

    def probe(self, command, env=None, **kwargs):
        """Run command(paths) inside a prepared sandbox; returns stdout."""
        options = self.options(**kwargs)
        producer = HERMETIC.Producer(options, dict(self.env, **(env or {})))
        try:
            work, plan = producer.prepare()
            out = Path(options.out) / "current"
            out.mkdir(parents=True)
            sandbox = producer.sandbox(plan, work, out)
            (work / "cargo").mkdir(exist_ok=True)
            HERMETIC.stage_cargo_home(plan, producer.host_cargo, work / "cargo")
            done = subprocess.run(sandbox.wrap(command(sandbox.paths)), cwd=str(plan.manifest.parent),
                                  env=sandbox.environment(), text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            self.assertEqual(done.returncode, 0, done.stderr)
            return done.stdout
        finally:
            if producer.temporary:
                shutil.rmtree(producer.work, ignore_errors=True)

    def write(self, name, content):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def final(self, receipt, index=0):
        return receipt["builds"][index]["final"]

    def binary(self, receipt, index=0, name="app"):
        out = Path(receipt["out"]) / ("a" if index == 0 else "b")
        return next(out.glob("*/" + name)).read_bytes()

    def test_double_build_is_reusable_and_keys_match(self):
        key, receipt = self.produce(profile="release", diagnose=True)
        self.assertIsNotNone(key, receipt)
        self.assertEqual(self.final(receipt, 0), self.final(receipt, 1))
        self.assertTrue(receipt["reusable"])
        self.assertEqual(sorted(receipt["document"]["trees"]),
                         ["dep", "plain", "pm", "ws/app", "ws/macro-user", "ws/other", "ws/ticking"])
        self.assertNotIn("generated", receipt["document"])
        again, _ = self.produce(profile="release", key_only=True)
        self.assertEqual(key, again)

    def test_clock_in_build_script_is_not_reusable(self):
        key, receipt = self.produce(package="ticking", profile="release")
        self.assertIsNotNone(key, receipt)
        self.assertFalse(receipt["reusable"])
        self.assertNotEqual(self.final(receipt, 0), self.final(receipt, 1))
        self.assertIn(b"zq7-", self.binary(receipt, 0, "ticking"))

    def test_environment_outside_allowlist_cannot_reach_the_build(self):
        key_a, receipt_a = self.produce(env={"PLAIN_NOTE": "zq7-alpha-marker"}, single=True)
        self.assertIsNotNone(key_a, receipt_a)
        binary_a = self.binary(receipt_a)
        key_b, receipt_b = self.produce(env={"PLAIN_NOTE": "zq7-beta-marker"}, single=True)
        binary_b = self.binary(receipt_b)
        self.assertEqual(key_a, key_b)
        self.assertNotIn("PLAIN_NOTE", receipt_a["document"]["env"])
        for binary in (binary_a, binary_b):
            self.assertIn(b"zq7-unset", binary)
            self.assertNotIn(b"zq7-alpha", binary)
            self.assertNotIn(b"zq7-beta", binary)
        if LINUX:
            self.assertEqual(self.final(receipt_a), self.final(receipt_b))

    def test_host_home_state_cannot_reach_the_build(self):
        home = Path(self.scratch.name) / "host-home"
        (home / ".cargo").mkdir(parents=True)
        (home / ".cargo" / "config.toml").write_text('[build]\nrustflags = ["--cfg", "zq7_marker"]\n')
        (home / "zq7-marker").write_text("x")
        plain, receipt_plain = self.produce(single=True)
        key, receipt = self.produce(env={"HOME": str(home)}, single=True)
        self.assertEqual(plain, key)
        listing = self.probe(lambda paths: ["ls", "-A", paths["home"]], env={"HOME": str(home)})
        self.assertEqual(listing.strip(), "")
        if LINUX:
            self.assertEqual(self.final(receipt_plain), self.final(receipt))

    def test_registry_state_is_verified_and_minimal(self):
        key, receipt = self.produce(single=True)
        self.assertIsNotNone(key, receipt)
        cargo_home = Path(self.env["CARGO_HOME"])
        unpacked = next(cargo_home.glob("registry/src/*/cfg-if-*"))
        (unpacked / "zq7-marker").write_text("x")
        self.addCleanup((unpacked / "zq7-marker").unlink)
        listing = self.probe(lambda paths: ["sh", "-c", "ls -A %s/registry/src/*/ && ls -A %s/registry/src/*/cfg-if-*/"
                                            % (paths["cargo"], paths["cargo"])])
        self.assertNotIn("zq7-marker", listing)
        self.assertNotIn("either", listing)
        self.assertIn("cfg-if-", listing)
        same, receipt_again = self.produce(single=True)
        self.assertEqual(key, same)
        if LINUX:
            self.assertEqual(self.final(receipt), self.final(receipt_again))
        crate = next(cargo_home.glob("registry/cache/*/cfg-if-*.crate"))
        original = crate.read_bytes()
        crate.write_bytes(original + b"\0")
        self.addCleanup(crate.write_bytes, original)
        miss, reason = self.produce(single=True)
        self.assertIsNone(miss)
        self.assertIn("does not match Cargo.lock", reason)

    def test_git_dependency_is_verified_at_locked_revision(self):
        key, receipt = self.produce(single=True)
        self.assertIsNotNone(key, receipt)
        self.assertTrue(any(BK.parse_package_id(p)[1] == "gitdep" for p in receipt["document"]["lock"]))
        checkout = next(Path(self.env["CARGO_HOME"]).glob("git/checkouts/gitdep-*/*/src/lib.rs"))
        original = checkout.read_text()
        checkout.write_text("pub fn h() -> u32 { 5 }\n")
        self.addCleanup(checkout.write_text, original)
        miss, reason = self.produce(single=True)
        self.assertIsNone(miss)
        self.assertIn("is modified", reason)

    @unittest.skipUnless(LINUX, "macOS cannot remount; its canonical paths are fixed host paths")
    def test_host_work_dir_does_not_change_bytes_or_key(self):
        first, receipt_1 = self.produce(single=True, work=str(Path(self.scratch.name) / "work-one"))
        second, receipt_2 = self.produce(single=True, work=str(Path(self.scratch.name) / "work-two"))
        self.assertEqual(first, second, receipt_2)
        self.assertEqual(self.final(receipt_1), self.final(receipt_2))
        self.assertEqual(receipt_1["document"]["sandbox"]["paths"]["src"], "/build/src")

    @unittest.skipUnless(LINUX, "pinned sysroot images are a Linux producer feature")
    def test_sysroot_is_pinned_and_verified(self):
        rootfs = Path(SYSROOT)
        inside = self.probe(lambda paths: ["sh", "-c", "ls -A /etc"]).split()
        self.assertEqual(sorted(inside), sorted(os.listdir(rootfs / "etc")))
        self.assertNotEqual(sorted(inside), sorted(os.listdir("/etc")))
        key, receipt = self.produce(key_only=True)
        self.assertEqual(receipt["document"]["sandbox"]["sysroot"],
                         json.loads((rootfs / HERMETIC.SYSROOT_SEAL).read_text())["image"])
        marker = rootfs / "etc" / "zq7-marker"
        marker.write_text("drift")
        self.addCleanup(marker.unlink)
        miss, reason = self.produce(key_only=True)
        self.assertIsNone(miss)
        self.assertIn("drifted from its seal", reason)

    def test_existing_output_dir_is_refused(self):
        options = self.options(single=True)
        (Path(options.out) / "a").mkdir(parents=True)
        try:
            HERMETIC.produce(options, dict(self.env))
            self.fail("expected a miss")
        except BK.Miss as miss:
            self.assertIn("exists", str(miss))

    def test_key_only_equals_built_key(self):
        computed, _ = self.produce(key_only=True)
        built, receipt = self.produce(single=True)
        self.assertEqual(computed, built, receipt)

    def test_allowlisted_environment_changes_key(self):
        before, _ = self.produce(key_only=True)
        versioned, receipt = self.produce(key_only=True, env={"ELASTOS_RELEASE_VERSION": "1.2.3"})
        self.assertEqual(receipt["document"]["env"]["ELASTOS_RELEASE_VERSION"], "1.2.3")
        self.assertNotEqual(before, versioned)
        flags, _ = self.produce(key_only=True, env={"RUSTFLAGS": "-C opt-level=1"})
        self.assertNotEqual(before, flags)

    def test_proc_macro_undeclared_file_read_is_miss(self):
        key, reason = self.produce(package="macro-user", single=True)
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)
        self.write("scripts/build-key-edges.json", json.dumps(
            {"packages": {"app": ["assets/a.txt"], "pm": ["assets/pm.txt"]}, "units": {}}))
        self.commit("declare pm edge")
        declared, receipt = self.produce(package="macro-user", single=True)
        self.assertIsNotNone(declared, receipt)
        self.assertIn("assets/pm.txt", receipt["document"]["files"])
        self.write("assets/pm.txt", "macro input changed\n")
        self.commit("change pm input")
        changed, _ = self.produce(package="macro-user", key_only=True)
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
        key, reason = self.produce(single=True)
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_undeclared_cross_package_include_is_miss(self):
        self.write("scripts/build-key-edges.json", json.dumps({"packages": {}, "units": {}}))
        self.commit("drop declared edge")
        key, reason = self.produce(single=True)
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_absolute_read_outside_sandbox_is_miss(self):
        outside = Path(self.scratch.name) / "outside.txt"
        outside.write_text("x")
        self.write("plain/build.rs", FILES["plain/build.rs"].replace(
            'let note = read_env("PLAIN_NOTE").unwrap_or_else(|_| "zq7-unset".to_string());',
            'let note = std::fs::read_to_string(%s).unwrap();' % json.dumps(str(outside))))
        self.commit("read outside")
        key, reason = self.produce(single=True)
        self.assertIsNone(key)
        self.assertIn("hermetic build failed", reason)

    def test_new_integration_test_changes_key(self):
        before, _ = self.produce(key_only=True)
        self.write("ws/app/tests/extra.rs", "#[test] fn extra() {}\n")
        self.commit("add integration test")
        after, _ = self.produce(key_only=True)
        self.assertNotEqual(before, after)

    def test_dependency_source_byte_changes_key(self):
        before, _ = self.produce(key_only=True)
        self.write("dep/src/lib.rs", "pub fn f() -> u32 { 2 }\n")
        self.commit("change dep")
        after, _ = self.produce(key_only=True)
        self.assertNotEqual(before, after)

    def test_edges_map_change_changes_key(self):
        before, receipt = self.produce(key_only=True)
        self.assertEqual(receipt["document"]["edges"], BK.git_blob(self.root / "scripts/build-key-edges.json"))
        self.write("scripts/build-key-edges.json", json.dumps(
            {"packages": {"app": ["assets/a.txt"]}, "units": {"app/bin": {"paths": ["assets/pm.txt"]}}}))
        self.commit("add unit edge")
        after, receipt = self.produce(key_only=True)
        self.assertNotEqual(before, after)
        self.assertIn("assets/pm.txt", receipt["document"]["files"])

    def test_lock_slice_covers_closure_only_and_corrupt_lock_is_miss(self):
        _, receipt = self.produce(key_only=True)
        document = receipt["document"]
        self.assertEqual(sorted(BK.parse_package_id(p)[1] for p in document["lock"]), ["cfg-if", "gitdep"])
        self.assertNotIn("path:ws/other#0.1.0", document["closure"])
        lock = self.root / "ws/Cargo.lock"
        blocks = lock.read_text().split("[[package]]")
        either = next(b for b in blocks if 'name = "either"' in b)
        checksum = either.split('checksum = "')[1].split('"')[0]
        self.assertNotIn(checksum, BK.canonical(document))
        lock.write_text("[[package]]".join(b.replace(checksum, "0000" + checksum[4:]) for b in blocks))
        self.commit("corrupt unrelated lock entry")
        key, reason = self.produce(key_only=True)
        self.assertIsNone(key)
        self.assertIn("checksum for `either", reason)

    def test_dirty_worktree_is_refused(self):
        self.write("assets/a.txt", "dirty\n")
        key, reason = self.produce(key_only=True)
        self.assertIsNone(key)
        self.assertIn("uncommitted changes", reason)

    def test_test_unit_includes_dev_dependencies(self):
        manifest = self.root / "ws/app/Cargo.toml"
        self.write("ws/app/Cargo.toml", manifest.read_text() + '[dev-dependencies]\nother = { path = "../other" }\n')
        subprocess.run(["cargo", "generate-lockfile", "-q"], cwd=str(self.root / "ws"), env=self.env, check=True)
        self.commit("dev dependency")
        _, bin_receipt = self.produce(key_only=True)
        _, test_receipt = self.produce(key_only=True, kind="test")
        self.assertNotIn("path:ws/other#0.1.0", bin_receipt["document"]["closure"])
        self.assertIn("path:ws/other#0.1.0", test_receipt["document"]["closure"])
        self.assertIn("either", [BK.parse_package_id(p)[1] for p in test_receipt["document"]["lock"]])


if __name__ == "__main__":
    if shutil.which("cargo") is None:
        print("cargo not found; hermetic tests need a Rust toolchain", file=sys.stderr)
        sys.exit(1)
    if LINUX and (shutil.which("bwrap") is None or not SYSROOT):
        print("Linux needs bwrap and HERMETIC_SYSROOT=<sealed rootfs>", file=sys.stderr)
        sys.exit(1)
    if not LINUX and shutil.which("sandbox-exec") is None:
        print("macOS needs sandbox-exec", file=sys.stderr)
        sys.exit(1)
    unittest.main()
