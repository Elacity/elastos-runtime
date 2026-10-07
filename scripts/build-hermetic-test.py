#!/usr/bin/env python3
"""Hermetic builds of a committed fixture workspace: every known false hit must miss or change the key.

Linux needs bwrap and HERMETIC_SYSROOT=<sealed rootfs> (see build-hermetic.py make-sysroot);
macOS needs sandbox-exec.
"""
import importlib.util
import json
import os
from pathlib import Path
import re
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


def link_tree(source, target):
    """Hard-link copy of a tree (same bytes, no space): a safe place to tamper with a toolchain or rootfs."""
    for directory, dirnames, filenames in os.walk(source):
        rel = os.path.relpath(directory, source)
        dest = os.path.join(target, rel) if rel != "." else target
        os.makedirs(dest, exist_ok=True)
        os.chmod(dest, os.lstat(directory).st_mode & 0o7777)
        for name in dirnames + filenames:
            path = os.path.join(directory, name)
            if os.path.islink(path):
                os.symlink(os.readlink(path), os.path.join(dest, name))
            elif os.path.isfile(path):
                try:
                    os.link(path, os.path.join(dest, name))
                except OSError:  # another filesystem: copy instead
                    shutil.copy2(path, os.path.join(dest, name))
        dirnames[:] = [d for d in dirnames if not os.path.islink(os.path.join(directory, d))]


def replace_file(path, content):
    """Replace a (possibly hard-linked) file with new content without touching the original."""
    os.unlink(path)
    Path(path).write_bytes(content)


def package(name, extra=""):
    return textwrap.dedent("""\
        [package]
        name = "%s"
        version = "0.1.0"
        edition = "2021"
        """ % name) + extra


FILES = {
    ".gitignore": "target/\n",
    "ws/Cargo.toml": '[workspace]\nmembers = ["app", "other", "macro-user", "ticking", "probing", "stamping", "delegating",'
                     ' "linking", "versioned"]\nresolver = "2"\n',
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
    # A build script that reads host CPU facts: bytes may follow the producer's CPU.
    "ws/probing/Cargo.toml": package("probing", "build = \"build.rs\"\n"),
    "ws/probing/build.rs": textwrap.dedent("""\
        fn main() {
            let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
            println!("cargo:rustc-env=PROBE_FLAGS={}", info.lines().filter(|l| l.starts_with("flags")).count());
        }
        """),
    "ws/probing/src/main.rs": 'fn main() { println!("{}", env!("PROBE_FLAGS")); }\n',
    # A build script that stamps a source file's mtime into the binary.
    "ws/stamping/Cargo.toml": package("stamping", "build = \"build.rs\"\n"),
    "ws/stamping/build.rs": textwrap.dedent("""\
        fn main() {
            let modified = std::fs::metadata("src/main.rs").unwrap().modified().unwrap();
            let secs = modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            println!("cargo:rustc-env=SRC_STAMP=zq7-stamp-{}", secs);
        }
        """),
    "ws/stamping/src/main.rs": 'fn main() { println!("{}", env!("SRC_STAMP")); }\n',
    # A build script whose CPU probe hides in a helper module the scan does not see.
    "ws/delegating/Cargo.toml": package("delegating", "build = \"build.rs\"\n"),
    "ws/delegating/build.rs": 'mod helper;\nfn main() { println!("cargo:rustc-env=DELEGATED={}", helper::probe()); }\n',
    "ws/delegating/helper.rs": textwrap.dedent("""\
        pub fn probe() -> usize {
            let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
            info.lines().filter(|l| l.starts_with("flags")).count()
        }
        """),
    "ws/delegating/src/main.rs": 'fn main() { println!("{}", env!("DELEGATED")); }\n',
    # A build script that stamps a symlink's own (lstat) mtime; link.rs -> src/main.rs is created in setUpClass.
    "ws/linking/Cargo.toml": package("linking", "build = \"build.rs\"\n"),
    "ws/linking/build.rs": textwrap.dedent("""\
        fn main() {
            let modified = std::fs::symlink_metadata("link.rs").unwrap().modified().unwrap();
            let secs = modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
            println!("cargo:rustc-env=LINK_STAMP=zq7-link-{}", secs);
        }
        """),
    "ws/linking/src/main.rs": 'fn main() { println!("{}", env!("LINK_STAMP")); }\n',
    # Mirrors elastos-server/build.rs: unset, empty and literal values must build (and key) differently.
    "ws/versioned/Cargo.toml": package("versioned", "build = \"build.rs\"\n"),
    "ws/versioned/build.rs": textwrap.dedent("""\
        fn main() {
            println!("cargo:rerun-if-env-changed=ELASTOS_RELEASE_VERSION");
            let version = std::env::var("ELASTOS_RELEASE_VERSION").unwrap_or_else(|_| "zq7-dev".to_string());
            println!("cargo:rustc-env=ELASTOS_VERSION=<{}>", version);
        }
        """),
    "ws/versioned/src/main.rs": 'fn main() { println!("{}", env!("ELASTOS_VERSION")); }\n',
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
        os.symlink("src/main.rs", cls.root / "ws" / "linking" / "link.rs")
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
            plan, stage, digests, _ = producer.prepare()
            out = Path(options.out) / "current"
            out.mkdir(parents=True)
            sandbox = producer.sandbox(stage, out)
            done = subprocess.run(sandbox.wrap(command(sandbox.paths), producer.workdir(plan, sandbox)),
                                  cwd=str(self.root), env=sandbox.environment(plan), text=True,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE)
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

    def test_double_build_agrees_and_keys_match(self):
        key, receipt = self.produce(profile="release", diagnose=True)
        self.assertIsNotNone(key, receipt)
        self.assertEqual(self.final(receipt, 0), self.final(receipt, 1))
        self.assertIsNone(receipt["reusable"])
        self.assertIn("attest", receipt["reason"])
        document = receipt["document"]
        for name in ("src", "cargo"):
            self.assertRegex(document[name], r"^[0-9a-f]{64}$")
        self.assertRegex(document["toolchain"]["digest"], r"^[0-9a-f]{64}$")
        self.assertNotIn("trees", document)
        self.assertNotIn("lock", document)
        again, _ = self.produce(profile="release", key_only=True)
        self.assertEqual(key, again)

    def test_clock_in_build_script_is_not_reusable(self):
        key, receipt = self.produce(package="ticking", profile="release")
        self.assertIsNotNone(key, receipt)
        self.assertFalse(receipt["reusable"])
        self.assertIn("differ", receipt["reason"])
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

    def test_git_dependency_is_staged_at_locked_revision(self):
        key, receipt = self.produce(single=True)
        self.assertIsNotNone(key, receipt)
        self.assertIn(b"4", self.binary(receipt))
        listing = self.probe(lambda paths: ["cat", paths["cargo"] + "/git/checkouts/" + next(
            p.name for p in Path(self.env["CARGO_HOME"]).glob("git/checkouts/gitdep-*")) + "/" + next(
            p.name for p in Path(self.env["CARGO_HOME"]).glob("git/checkouts/gitdep-*/*")) + "/src/lib.rs"])
        self.assertEqual(listing, GITDEP["src/lib.rs"])
        # The host checkout is never staged (only git archive of the locked revision is), so a
        # modified host checkout changes neither the key nor the bytes.
        checkout = next(Path(self.env["CARGO_HOME"]).glob("git/checkouts/gitdep-*/*/src/lib.rs"))
        original = checkout.read_text()
        checkout.write_text("pub fn h() -> u32 { 5 }\n")
        self.addCleanup(checkout.write_text, original)
        same, receipt_same = self.produce(single=True)
        self.assertEqual(key, same, receipt_same)
        if LINUX:
            self.assertEqual(self.final(receipt), self.final(receipt_same))

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
        seal = json.loads((rootfs / HERMETIC.SYSROOT_SEAL).read_text())
        self.assertEqual(receipt["document"]["sysroot"], {"image": seal["image"], "digest": seal["digest"]})
        marker = rootfs / "etc" / "zq7-marker"
        marker.write_text("drift")
        self.addCleanup(HERMETIC.canonicalize, rootfs)  # restore /etc's mtime after the marker is gone
        self.addCleanup(marker.unlink)
        miss, reason = self.produce(key_only=True)
        self.assertIsNone(miss)
        self.assertIn("drifted from its seal", reason)

    def test_toolchain_is_exposed_minimally_and_keyed_by_content(self):
        real = Path(HERMETIC.sh(["rustc", "--print", "sysroot"], cwd=self.root / "ws", env=self.env).stdout.strip()).resolve()
        rustup = Path(os.path.realpath(self.env.get("RUSTUP_HOME") or Path.home() / ".rustup"))
        copy = Path(self.scratch.name) / "rustup-copy"
        link_tree(real, copy / "toolchains" / real.name)
        shutil.copyfile(rustup / "settings.toml", copy / "settings.toml")
        (copy / "zq7-marker").write_text("host rustup state")
        env = {"RUSTUP_HOME": str(copy)}
        key, receipt = self.produce(key_only=True, env=env)
        self.assertIsNotNone(key, receipt)
        plain, _ = self.produce(key_only=True)
        self.assertEqual(key, plain, "a hard-link copy of the toolchain stages to the same bytes")
        listing = self.probe(lambda paths: ["sh", "-c", "ls -A %s && cat %s/../../zq7-marker 2>&1; true"
                                            % (paths["toolchain"], paths["toolchain"])], env=env)
        self.assertIn("bin", listing)
        self.assertNotIn("host rustup state", listing)
        bookkeeping = copy / "toolchains" / real.name / "lib" / "rustlib" / "components"
        replace_file(bookkeeping, bookkeeping.read_bytes() + b"zq7\n")
        same, _ = self.produce(key_only=True, env=env)
        self.assertEqual(key, same, "rustup's install records are not staged")
        target = copy / "toolchains" / real.name / "lib" / "rustlib" / "etc" / "rust_types.py"
        replace_file(target, target.read_bytes() + b"# zq7\n")
        changed, receipt = self.produce(key_only=True, env=env)
        self.assertNotEqual(key, changed)

    def test_registry_index_is_generated_from_locked_lines_only(self):
        cargo_home = Path(self.env["CARGO_HOME"])
        arbitrary = cargo_home / "registry" / "index" / "arbitrary"
        arbitrary.mkdir(parents=True)
        (arbitrary / "config.json").write_text('{"dl": "http://zq7.invalid"}')
        self.addCleanup(shutil.rmtree, arbitrary)
        key, receipt = self.produce(key_only=True)
        self.assertIsNotNone(key, receipt)
        real = next(cargo_home.glob("registry/index/index.*"))
        listing = self.probe(lambda paths: ["sh", "-c", "ls -A %s/registry/index && find %s/registry/index -type f"
                                            " && cat %s/registry/index/*/.cache/cf/g-/cfg-if | tr '\\0' '\\n'"
                                            % (paths["cargo"], paths["cargo"], paths["cargo"])])
        self.assertNotIn("arbitrary", listing)
        self.assertEqual(sorted(Path(l).name for l in listing.split("\n") if l.startswith("/")),
                         ["cfg-if", "config.json", "either"])
        self.assertEqual(listing.count('"name":"cfg-if"'), 1, "the staged entry holds exactly the locked version")
        config = real / "config.json"
        original = config.read_bytes()
        config.write_bytes(original.replace(b"}", b", \"zq7\": 1}"))
        self.addCleanup(config.write_bytes, original)
        changed, _ = self.produce(key_only=True)
        self.assertNotEqual(key, changed)
        config.write_bytes(original)
        entry = real / ".cache" / HERMETIC.index_cache_path("cfg-if")
        original_entry = entry.read_bytes()
        self.addCleanup(entry.write_bytes, original_entry)
        # Astra round 5: an unlocked version's entry changes; staged bytes and key must not.
        parts = original_entry.split(b"\0")
        fake = [b"9.9.9", b'{"name":"cfg-if","vers":"9.9.9","deps":[],"cksum":"00","features":{},"yanked":false}']
        parts = parts[:-1] + fake + parts[-1:] if parts[-1] == b"" else parts + fake
        entry.write_bytes(b"\0".join(parts))
        same, receipt_same = self.produce(key_only=True)
        self.assertEqual(key, same, receipt_same)
        self.assertEqual(receipt["document"]["cargo"], receipt_same["document"]["cargo"])
        entry.write_bytes(original_entry)
        locked = next(v for v in listing.split("\n") if re.fullmatch(r"\d+\.\d+\.\d+", v))
        line = HERMETIC.index_entry_line(entry, locked)
        self.assertIsNotNone(line, locked)
        entry.write_bytes(original_entry.replace(line, line.replace(b'"cksum":"', b'"cksum":"0000')))
        miss, reason = self.produce(key_only=True)
        self.assertIsNone(miss)
        self.assertTrue("checksum" in reason or "cksum" in reason, reason)

    @unittest.skipUnless(LINUX, "pinned sysroot images are a Linux producer feature")
    def test_sysroot_key_follows_sealed_content(self):
        copy = Path(self.scratch.name) / "sysroot-copy"
        link_tree(Path(SYSROOT), copy)
        (copy / HERMETIC.SYSROOT_SEAL).unlink()
        HERMETIC.seal_sysroot(copy, "sha256:test")
        key, receipt = self.produce(key_only=True, sysroot=str(copy))
        self.assertIsNotNone(key, receipt)
        self.assertEqual(receipt["document"]["sysroot"]["digest"],
                         json.loads((copy / HERMETIC.SYSROOT_SEAL).read_text())["digest"])
        probe = copy / "etc" / "passwd"
        replace_file(probe, b"zq7\n")
        HERMETIC.seal_sysroot(copy, "sha256:test")
        resealed, _ = self.produce(key_only=True, sysroot=str(copy))
        self.assertNotEqual(key, resealed)
        os.chmod(probe, 0o755)  # the executable bit survives canonicalization, so it is content
        miss, reason = self.produce(key_only=True, sysroot=str(copy))
        self.assertIsNone(miss)
        self.assertIn("drifted from its seal", reason)
        HERMETIC.seal_sysroot(copy, "sha256:test")
        remoded, _ = self.produce(key_only=True, sysroot=str(copy))
        self.assertNotEqual(resealed, remoded)
        (copy / "etc" / "zq7-empty").mkdir()
        miss, reason = self.produce(key_only=True, sysroot=str(copy))
        self.assertIn("drifted from its seal", reason)

    def test_host_cpu_flags_are_refused_and_target_cfg_keyed(self):
        for flags in ("-C target-cpu=native", "-Ctarget-feature=+avx2", "--codegen target-cpu=haswell"):
            key, reason = self.produce(key_only=True, env={"RUSTFLAGS": flags})
            self.assertIsNone(key, flags)
            self.assertIn("host-CPU codegen flags are not allowed", reason)
        self.write("ws/.cargo/config.toml", '[build]\nrustflags = ["-C", "target-cpu=native"]\n')
        self.commit("native cpu in config")
        key, reason = self.produce(key_only=True)
        self.assertIsNone(key)
        self.assertIn("target-cpu=native", reason)
        self.git(self.root, "reset", "-q", "--hard", "HEAD~1")
        key, receipt = self.produce(key_only=True)
        self.assertIsNotNone(key, receipt)
        cfg = receipt["document"]["target_cfg"]
        self.assertTrue(any(line.startswith("target_arch=") for line in cfg), cfg)
        self.assertTrue(any(line.startswith("target_feature=") for line in cfg), cfg)
        if LINUX:
            self.assertEqual(self.probe(lambda paths: ["cat", "/proc/cpuinfo"]), "processor\t: 0\nflags\t\t:\n")

    def test_build_time_cpu_detection_is_not_reusable(self):
        key, receipt = self.produce(package="probing", profile="release")
        self.assertIsNotNone(key, receipt)
        self.assertEqual(receipt["cpu_detection"], ["probing"])
        self.assertFalse(receipt["reusable"])
        self.assertEqual(self.final(receipt, 0), self.final(receipt, 1))
        _, plain = self.produce(key_only=True)
        self.assertEqual(plain["cpu_detection"], [])

    def fake_receipt(self, receipt, cpu, finals=None, run=None):
        other = json.loads(json.dumps(receipt))
        other["host"]["cpu"] = cpu
        other["run"] = run or "fake-%d" % (type(self).runs + 1)
        if finals is not None:
            for build in other["builds"]:
                build["final"] = finals
        type(self).runs += 1
        path = Path(self.scratch.name) / ("receipt-%d.json" % type(self).runs)
        path.write_text(json.dumps(other))
        return str(path)

    def test_helper_module_cpu_probe_needs_a_second_host(self):
        key, receipt = self.produce(package="delegating", profile="release")
        self.assertIsNotNone(key, receipt)
        self.assertEqual(receipt["cpu_detection"], [], "the scan cannot see helper.rs; attestation must")
        self.assertIsNone(receipt["reusable"])
        self.assertRegex(receipt["host"]["cpu"], r"\S")
        mine = self.fake_receipt(receipt, receipt["host"]["cpu"])
        same_cpu = self.fake_receipt(receipt, receipt["host"]["cpu"])
        other_cpu = self.fake_receipt(receipt, "zq7 other cpu stepping 9 flags 000000000000")
        differing = self.fake_receipt(receipt, "zq7 other cpu", finals={"release/delegating": "sha256:zq7"})
        self.assertFalse(HERMETIC.attest([mine])[0])
        verdict, reason = HERMETIC.attest([mine, same_cpu])
        self.assertFalse(verdict)
        self.assertIn("same CPU", reason)
        verdict, reason = HERMETIC.attest([mine, differing])
        self.assertFalse(verdict)
        self.assertIn("differ", reason)
        verdict, reason = HERMETIC.attest([mine, other_cpu])
        self.assertTrue(verdict, reason)

    def test_attest_rejects_duplicate_and_incomplete_receipts(self):
        key, receipt = self.produce(package="delegating", profile="release")
        self.assertIsNotNone(key, receipt)
        mine = self.fake_receipt(receipt, receipt["host"]["cpu"], run=receipt["run"])
        copy = Path(self.scratch.name) / "receipt-copy.json"
        shutil.copyfile(mine, copy)
        _, key_only = self.produce(package="delegating", profile="release", key_only=True)
        other_cpu = "zq7 other cpu stepping 9 flags 000000000000"
        key_only_b = self.fake_receipt(key_only, other_cpu)
        _, single = self.produce(package="delegating", profile="release", single=True)
        single_b = self.fake_receipt(single, other_cpu)
        same_run_b = self.fake_receipt(receipt, other_cpu, run=receipt["run"])
        complete_b = self.fake_receipt(receipt, other_cpu)
        for receipts, expected in (([mine, str(copy), key_only_b], "duplicate receipt"),
                                   ([mine, str(copy)], "duplicate receipt"),
                                   ([mine, same_run_b], "duplicate receipt"),
                                   ([mine, key_only_b], "not a complete double build"),
                                   ([mine, single_b], "not a complete double build"),
                                   ([key_only_b, complete_b], "not a complete double build"),
                                   ([mine], "two hosts")):
            verdict, reason = HERMETIC.attest(receipts)
            self.assertFalse(verdict, (receipts, reason))
            self.assertIn(expected, reason)
        verdict, reason = HERMETIC.attest([mine, complete_b])
        self.assertTrue(verdict, reason)

    def test_symlink_mtimes_are_canonical(self):
        key, receipt = self.produce(package="linking", single=True)
        self.assertIsNotNone(key, receipt)
        self.assertIn(b"zq7-link-%d" % HERMETIC.SOURCE_DATE_EPOCH, self.binary(receipt, 0, "linking"))
        self.git(self.root, "commit", "-q", "--amend", "--no-edit", "--date", "2003-04-05T06:07:08Z")
        later, receipt_later = self.produce(package="linking", single=True)
        self.assertEqual(key, later)
        self.assertIn(b"zq7-link-%d" % HERMETIC.SOURCE_DATE_EPOCH, self.binary(receipt_later, 0, "linking"))
        if LINUX:
            self.assertEqual(self.final(receipt), self.final(receipt_later))
        listing = self.probe(lambda paths: ["sh", "-c", "stat -c %%Y %s/ws/linking/link.rs 2>/dev/null || "
                                            "stat -f %%m %s/ws/linking/link.rs" % (paths["src"], paths["src"])])
        self.assertEqual(listing.strip(), str(HERMETIC.SOURCE_DATE_EPOCH))

    def test_sub_second_mtimes_are_keyed_and_refused(self):
        tree = Path(self.scratch.name) / "ns-tree"
        (tree / "d").mkdir(parents=True)
        (tree / "d" / "f").write_text("x")
        os.symlink("f", tree / "d" / "l")
        HERMETIC.canonicalize(tree)
        exact = HERMETIC.tree_digest(tree, strict=True)
        epoch_ns = HERMETIC.SOURCE_DATE_EPOCH * 1_000_000_000
        for target in (tree / "d" / "f", tree / "d", tree / "d" / "l"):
            os.utime(target, ns=(epoch_ns + 500_000_000, epoch_ns + 500_000_000), follow_symlinks=False)
            self.assertNotEqual(exact, HERMETIC.tree_digest(tree), target)
            with self.assertRaises(BK.Miss) as refused:
                HERMETIC.tree_digest(tree, strict=True)
            self.assertIn("1500000000 ns", str(refused.exception))
            os.utime(target, ns=(epoch_ns, epoch_ns), follow_symlinks=False)
        self.assertEqual(exact, HERMETIC.tree_digest(tree, strict=True))
        if LINUX:
            copy = Path(self.scratch.name) / "sysroot-ns"
            link_tree(Path(SYSROOT), copy)
            (copy / HERMETIC.SYSROOT_SEAL).unlink()
            HERMETIC.seal_sysroot(copy, "sha256:test")
            key, receipt = self.produce(key_only=True, sysroot=str(copy))
            self.assertIsNotNone(key, receipt)
            probe = copy / "etc" / "passwd"
            replace_file(probe, probe.read_bytes())
            os.utime(probe, ns=(epoch_ns + 500_000_000, epoch_ns + 500_000_000))
            os.utime(copy / "etc", ns=(epoch_ns, epoch_ns))  # the replacement touched the directory
            miss, reason = self.produce(key_only=True, sysroot=str(copy))
            self.assertIsNone(miss)
            self.assertIn("drifted from its seal", reason)
            self.assertIn("1500000000 ns", reason)
            # resealing cannot launder it either: the seal canonicalizes, so the digest is the exact one
            HERMETIC.seal_sysroot(copy, "sha256:test")
            self.assertEqual(os.lstat(probe).st_mtime_ns, epoch_ns)

    def test_digest_records_cannot_be_forged(self):
        # Astra round 8: a symlink target carrying a fake record must not collide with two real entries.
        epoch_ns = HERMETIC.SOURCE_DATE_EPOCH * 1_000_000_000
        forged = Path(self.scratch.name) / "forged"
        honest = Path(self.scratch.name) / "honest"
        forged.mkdir()
        honest.mkdir()
        os.symlink("x\nL b %d -> y" % epoch_ns, forged / "a")
        os.symlink("x", honest / "a")
        os.symlink("y", honest / "b")
        for tree in (forged, honest):
            HERMETIC.canonicalize(tree)
        self.assertNotEqual(HERMETIC.tree_digest(forged, strict=True), HERMETIC.tree_digest(honest, strict=True))
        # names and targets with quotes, newlines, JSON syntax and (where the filesystem allows) non-UTF-8 bytes
        odd = Path(self.scratch.name) / "odd"
        odd.mkdir()
        (odd / 'a"b,["c').write_text("1")
        (odd / "line\nbreak").write_text("2")
        try:
            os.symlink(b"\xff\xfe-target", os.fsencode(str(odd / "link")))
            (odd / os.fsdecode(b"\xff-name")).write_text("3")
        except (OSError, UnicodeError):
            pass
        HERMETIC.canonicalize(odd)
        first = HERMETIC.tree_digest(odd, strict=True)
        self.assertEqual(first, HERMETIC.tree_digest(odd, strict=True))
        os.rename(odd / 'a"b,["c', odd / 'a"b,["d')
        HERMETIC.canonicalize(odd)
        self.assertNotEqual(first, HERMETIC.tree_digest(odd, strict=True))
        self.assertEqual(HERMETIC.record_digest([["a", "b"]]), HERMETIC.record_digest([["a", "b"]]))
        self.assertNotEqual(HERMETIC.record_digest([["a", "b"]]), HERMETIC.record_digest([["a"], ["b"]]))
        self.assertNotEqual(HERMETIC.record_digest([["ab"]]), HERMETIC.record_digest([["a", "b"]]))

    def test_release_version_unset_empty_and_literal_key_and_build_differently(self):
        # Astra round 9: the key must hash exactly the environment the build runs with.
        results = {}
        for label, env in (("unset", {}), ("empty", {"ELASTOS_RELEASE_VERSION": ""}),
                           ("literal", {"ELASTOS_RELEASE_VERSION": "unversioned"})):
            key, receipt = self.produce(package="versioned", single=True, env=env)
            self.assertIsNotNone(key, receipt)
            results[label] = (key, receipt["document"]["env"], self.binary(receipt, 0, "versioned"))
        self.assertEqual(len({key for key, _, _ in results.values()}), 3)
        self.assertNotIn("ELASTOS_RELEASE_VERSION", results["unset"][1])
        self.assertEqual(results["empty"][1]["ELASTOS_RELEASE_VERSION"], "")
        self.assertEqual(results["literal"][1]["ELASTOS_RELEASE_VERSION"], "unversioned")
        self.assertIn(b"<zq7-dev>", results["unset"][2])
        self.assertIn(b"<>", results["empty"][2])
        self.assertIn(b"<unversioned>", results["literal"][2])

    def test_c_toolchain_cpu_flags_are_refused(self):
        for name, value in (("CFLAGS", "-O2 -march=native"), ("CXXFLAGS", "-mcpu=native"),
                            ("CPPFLAGS", "-march=x86-64-v3"), ("TARGET_CFLAGS", "-mtune=native")):
            key, reason = self.produce(key_only=True, env={name: value})
            self.assertIsNone(key, (name, value))
            self.assertIn("host-CPU codegen flags are not allowed", reason)
        key, receipt = self.produce(key_only=True, env={"CFLAGS": "-O2 -mtune=generic"})
        self.assertIsNotNone(key, receipt)

    def test_source_mtimes_are_canonical(self):
        key, receipt = self.produce(package="stamping", single=True)
        self.assertIsNotNone(key, receipt)
        self.assertIn(b"zq7-stamp-%d" % HERMETIC.SOURCE_DATE_EPOCH, self.binary(receipt, 0, "stamping"))
        self.git(self.root, "commit", "-q", "--amend", "--no-edit", "--date", "2001-02-03T04:05:06Z")
        later, receipt_later = self.produce(package="stamping", single=True)
        self.assertEqual(key, later)
        self.assertIn(b"zq7-stamp-%d" % HERMETIC.SOURCE_DATE_EPOCH, self.binary(receipt_later, 0, "stamping"))
        if LINUX:
            self.assertEqual(self.final(receipt), self.final(receipt_later))

    def test_git_dependency_has_no_git_dir_and_host_config_is_not_staged(self):
        key, receipt = self.produce(key_only=True)
        self.assertIsNotNone(key, receipt)
        listing = self.probe(lambda paths: ["sh", "-c", "find %s/git/checkouts -name .git; ls -A %s/git/checkouts/*/*/;"
                                            " git -C %s/git/db/* config --get-all remote.origin.url; true"
                                            % (paths["cargo"], paths["cargo"], paths["cargo"])])
        self.assertNotIn(".git", listing)
        self.assertIn("Cargo.toml", listing)
        self.assertNotIn(self.scratch.name, listing, "no host path leaks through the staged git database")
        db_config = next(Path(self.env["CARGO_HOME"]).glob("git/db/gitdep-*/config"))
        original = db_config.read_text()
        db_config.write_text(original + "[zq7]\n\tmarker = true\n")
        self.addCleanup(db_config.write_text, original)
        same, _ = self.produce(key_only=True)
        self.assertEqual(key, same)

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
        before, receipt = self.produce(key_only=True)
        self.assertNotIn("ELASTOS_RELEASE_VERSION", receipt["document"]["env"])
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
        self.assertEqual(self.probe(lambda paths: ["cat", paths["src"] + "/assets/pm.txt"], package="macro-user"),
                         "macro input\n")
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
        self.write("scripts/build-key-edges.json", json.dumps(
            {"packages": {"app": ["assets/a.txt"]}, "units": {"app/bin": {"paths": ["assets/pm.txt"]}}}))
        self.commit("add unit edge")
        after, receipt = self.produce(key_only=True)
        self.assertNotEqual(before, after)
        self.assertEqual(self.probe(lambda paths: ["cat", paths["src"] + "/assets/pm.txt"]), "macro input\n")

    def test_corrupt_lock_is_miss(self):
        lock = self.root / "ws/Cargo.lock"
        blocks = lock.read_text().split("[[package]]")
        either = next(b for b in blocks if 'name = "either"' in b)
        checksum = either.split('checksum = "')[1].split('"')[0]
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

    def test_test_unit_stages_dev_dependencies_only_for_test_units(self):
        manifest = self.root / "ws/app/Cargo.toml"
        self.write("ws/app/Cargo.toml", manifest.read_text() + '[dev-dependencies]\nother = { path = "../other" }\n')
        subprocess.run(["cargo", "generate-lockfile", "-q"], cwd=str(self.root / "ws"), env=self.env, check=True)
        self.commit("dev dependency")
        bin_key, _ = self.produce(key_only=True)
        test_key, _ = self.produce(key_only=True, kind="test")
        self.assertNotEqual(bin_key, test_key)
        crates = lambda paths: ["sh", "-c", "ls %s/registry/src/*/" % paths["cargo"]]
        self.assertNotIn("either", self.probe(crates))
        self.assertIn("either", self.probe(crates, kind="test"))


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
