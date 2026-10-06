#!/usr/bin/env python3
"""Exercise pre-push decisions with local Git fixtures and fake Cargo/Node."""
import fcntl
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import textwrap
import time
import unittest
from unittest import mock


SCRIPT = Path(__file__).with_name("ci-local-prepush.sh")
SPEC = importlib.util.spec_from_file_location("prepush", SCRIPT.with_suffix(".py"))
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


def fixture_environment(cwd):
    # Hooks export repository-local Git variables. Clear the complete Git-owned
    # list before init/add/commit can select an owned disposable repository.
    names = subprocess.check_output(["git", "rev-parse", "--local-env-vars"], cwd=cwd, text=True).splitlines()
    environment = {key: value for key, value in os.environ.items() if key not in names}
    environment.update({"GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": os.devnull,
                        "GIT_AUTHOR_NAME": "Fixture", "GIT_AUTHOR_EMAIL": "fixture@example.invalid",
                        "GIT_COMMITTER_NAME": "Fixture", "GIT_COMMITTER_EMAIL": "fixture@example.invalid"})
    return environment


class PrepushTests(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.tmp = Path(self.scratch.name).resolve()
        self.root = self.tmp / "source"
        self.root.mkdir()
        self.origin = self.tmp / "origin.git"
        self.log = self.tmp / "commands.jsonl"
        self.env = {**fixture_environment(self.tmp),
                    "PREPUSH_ROOT": str(self.root), "PREPUSH_LOG": str(self.log)}
        self.env.pop("CARGO_BUILD_BUILD_DIR", None)
        self.git("init", "-q", "-b", "develop")
        subprocess.run(["git", "init", "-q", "--bare", str(self.origin)], check=True, env=self.env)
        self.write("elastos/Cargo.toml", "[workspace]\n")
        self.write("elastos/crates/server/Cargo.toml", '[package]\nname = "server"\n')
        self.write("elastos/crates/server/src/lib.rs", "mod runtime;\n")
        self.write("elastos/crates/server/src/runtime.rs", "// fixture\n")
        self.write("elastos/crates/server/src/main.rs", "mod release_cmd;\n")
        self.write("elastos/crates/server/src/release_cmd.rs", "// fixture\n")
        self.write("elastos/crates/other/Cargo.toml", '[package]\nname = "other"\n')
        self.write("elastos/crates/common/Cargo.toml", '[package]\nname = "common"\n')
        self.write("elastos/crates/common/src/lib.rs", "// fixture\n")
        self.write("capsules/chain-provider/Cargo.toml", '[package]\nname = "chain-provider"\n')
        self.write("capsules/chain-provider/src/main.rs", "// fixture\n")
        self.write("capsules/wallet-provider/Cargo.toml", '[package]\nname = "wallet-provider"\n')
        self.write("capsules/wallet-provider/src/approval.rs", "// fixture\n")
        self.write("capsules/chat-room-ui/Cargo.toml", '[package]\nname = "chat-room-ui"\n')
        self.write("capsules/chat-room-ui/src/lib.rs", "// fixture\n")
        for name in ("protected-content-protect-provider", "protected-content-decrypt-provider", "custody-provider"):
            self.write("capsules/" + name + "/Cargo.toml", '[package]\nname = "' + name + '"\n')
        for workspace in ("elastos", "capsules/chain-provider", "capsules/wallet-provider",
                          "capsules/chat-room-ui", "capsules/protected-content-protect-provider",
                          "capsules/protected-content-decrypt-provider", "capsules/custody-provider"):
            self.write(workspace + "/Cargo.lock", "version = 4\n# committed fixture lock\n")
        self.write("README.md", "Fixture\n")
        self.write(".gitignore", "**/target/\n/target-build/\n")
        self.commit()
        self.git("remote", "add", "origin", str(self.origin))
        self.git("push", "-q", "origin", "develop")
        self.git("switch", "-q", "-c", "fix/fixture")
        self.write("elastos/crates/server/src/release_cmd.rs", "// changed module\n")
        self.commit()
        binaries = self.tmp / "bin"
        binaries.mkdir()
        self.env["PATH"] = str(binaries) + os.pathsep + self.env["PATH"]
        program = textwrap.dedent('''\
            import json, os, pathlib, signal, subprocess, sys, time
            root = pathlib.Path(os.environ["PREPUSH_ROOT"])
            args = sys.argv[1:]
            with open(os.environ["PREPUSH_LOG"], "a") as log:
                log.write(json.dumps({"tool": pathlib.Path(sys.argv[0]).name, "args": args,
                                      "cwd": os.getcwd(), "build_dir": os.environ.get("CARGO_BUILD_BUILD_DIR"),
                                      "target_dir": os.environ.get("CARGO_TARGET_DIR"),
                                      "build_target_dir": os.environ.get("CARGO_BUILD_TARGET_DIR"),
                                      "git_env": [k for k in os.environ if k.startswith("GIT_")],
                                      "provider_env": {k: v for k, v in os.environ.items() if k.startswith("ELASTOS_TEST_")}}) + "\\n")
            if pathlib.Path(sys.argv[0]).name == "node":
                sys.exit(0)
            if args[0] == "metadata":
                if os.environ.get("PREPUSH_IGNORE_TERM"):
                    signal.signal(signal.SIGTERM, signal.SIG_IGN)
                    pathlib.Path(os.environ["PREPUSH_IGNORE_TERM"]).write_text(str(os.getpid()))
                if os.environ.get("PREPUSH_CHILD"):
                    subprocess.Popen([sys.executable, "-c", "import os, pathlib, signal, time; signal.signal(signal.SIGTERM, signal.SIG_IGN); pathlib.Path(os.environ['PREPUSH_CHILD']).write_text(str(os.getpid())); time.sleep(30)"])
                if os.environ.get("PREPUSH_DETACHED"):
                    subprocess.Popen([sys.executable, "-c", "import os, pathlib, time; pathlib.Path(os.environ['PREPUSH_DETACHED']).write_text(str(os.getpid())); time.sleep(30)"],
                                     start_new_session=True, close_fds=False,
                                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if os.environ.get("PREPUSH_SLEEP"):
                    time.sleep(30)
                manifest = pathlib.Path(args[args.index("--manifest-path") + 1])
                workspace = manifest.parent
                if "--no-deps" not in args:
                    lock = workspace / "Cargo.lock"
                    relative = workspace.relative_to(root).as_posix()
                    if "--offline" in args and os.environ.get("PREPUSH_COLD_METADATA") == relative:
                        sys.exit("uncached metadata dependency requires a fetch")
                    update = os.environ.get("PREPUSH_UPDATE_LOCK") == relative
                    if "--locked" in args and (not lock.exists() or update):
                        sys.exit("workspace lock needs an update but --locked was supplied")
                    if not lock.exists() or update:
                        lock.write_text("version = 4\\n# generated fixture lock\\n")
                def package(name, folder, kinds):
                    graph = json.loads(os.environ.get("PREPUSH_GRAPH", "{}"))
                    dependencies = []
                    for dependency in graph.get(folder.relative_to(root).as_posix(), []):
                        dependency = {"path": dependency} if isinstance(dependency, str) else dependency.copy()
                        dependency["path"] = str(root / dependency["path"])
                        dependencies.append(dependency)
                    return {"id": name, "name": name, "source": None,
                            "manifest_path": str(folder / "Cargo.toml"),
                            "dependencies": dependencies,
                            "targets": [{"kind": [kind], "name": name, "test": True,
                                         "src_path": str(folder / "src" / ("main.rs" if kind == "bin" else "lib.rs"))} for kind in kinds]}
                if workspace == root / "elastos":
                    packages = [package(os.environ.get("PREPUSH_PACKAGE", "server"), workspace / "crates/server", ["lib", "bin"]),
                                package("other", workspace / "crates/other", ["lib"]),
                                package("common", workspace / "crates/common", ["lib"])]
                    packages[0]["targets"].extend(json.loads(os.environ.get("PREPUSH_LOCAL_TARGETS", "[]")))
                else:
                    packages = [package(workspace.name, workspace, ["cdylib" if workspace.name == "chat-room-ui" else "bin"])]
                members = [p["id"] for p in packages]
                if "--no-deps" not in args:
                    seen = {pathlib.Path(p["manifest_path"]).parent for p in packages}
                    pending = list(packages)
                    while pending:
                        for dependency in pending.pop()["dependencies"]:
                            folder = pathlib.Path(dependency["path"])
                            if folder in seen or not folder.is_relative_to(root):
                                continue
                            seen.add(folder)
                            name = os.environ.get("PREPUSH_PACKAGE", "server") if folder == root / "elastos/crates/server" else folder.name
                            value = package(name, folder, ["lib"] if folder.is_relative_to(root / "elastos/crates") else ["bin"])
                            packages.append(value)
                            pending.append(value)
                    packages.extend(json.loads(os.environ.get("PREPUSH_DEPENDENCY_PACKAGES", "[]")))
                print(json.dumps({"workspace_root": str(workspace), "workspace_members": members,
                                  "packages": packages}))
            elif args[0] == "check":
                if os.environ.get("PREPUSH_PRODUCT_SENTINEL"):
                    subprocess.run(["git", "-C", os.environ["PREPUSH_PRODUCT_SENTINEL"],
                                    "commit", "--allow-empty", "-qm", "owned product-child fixture"], check=True)
                if os.environ.get("PREPUSH_DIRTY"):
                    (root / "README.md").write_text("changed during check\\n")
                if os.environ.get("PREPUSH_HEAD"):
                    subprocess.run(["git", "commit", "--allow-empty", "-qm", "new candidate"], cwd=root, check=True)
                if os.environ.get("PREPUSH_ADVANCE"):
                    remote = os.environ["PREPUSH_ADVANCE"]
                    old = subprocess.check_output(["git", "-C", remote, "rev-parse", "refs/heads/develop"], text=True).strip()
                    tree = subprocess.check_output(["git", "-C", remote, "rev-parse", old + "^{tree}"], text=True).strip()
                    commit = subprocess.check_output(["git", "-C", remote, "commit-tree", tree, "-p", old, "-m", "base advanced"], text=True).strip()
                    subprocess.run(["git", "-C", remote, "update-ref", "refs/heads/develop", commit], check=True)
                if os.environ.get("PREPUSH_FAIL"):
                    sys.exit(1)
            elif args[0] == "build":
                binary = pathlib.Path(os.getcwd()) / "target/release" / pathlib.Path(os.getcwd()).name
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_text("fixture")
                binary.chmod(0o755)
            elif args[0] == "test":
                if "--list" in args:
                    empty = (os.environ.get("PREPUSH_NO_TESTS") or
                             os.environ.get("PREPUSH_EMPTY_PACKAGE") == args[args.index("-p") + 1] or
                             (os.environ.get("PREPUSH_EMPTY_BIN") and "--bin" in args))
                    listed = os.environ.get("PREPUSH_TESTS", "release_cmd::tests::updates: test")
                    if "--lib" in args:
                        listed = os.environ.get("PREPUSH_LIB_TESTS", listed)
                    print(listed if not empty else "0 tests")
                else:
                    count = 0 if os.environ.get("PREPUSH_ZERO") else 1
                    print(f"test result: ok. {count} passed; 0 failed; 1 ignored; 0 measured; 0 filtered out")
            ''')
        for tool in ("cargo", "node"):
            path = binaries / tool
            path.write_text("#!" + sys.executable + "\n" + program)
            path.chmod(0o755)

    def write(self, path, text):
        target = self.root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def git(self, *args):
        return subprocess.check_output(["git", *args], cwd=self.root, env=self.env, text=True).strip()

    def commit(self):
        self.git("add", "-A")
        self.git("commit", "-qm", "fixture")

    def invoke(self, input=None, extra=None):
        args = [str(SCRIPT)] + (["origin", str(self.origin)] if input is not None else [])
        return subprocess.run(args, cwd=self.root, env={**self.env, **(extra or {})},
                              input=input, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)

    def commands(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def dependency_package(self, name, source, folder=None, target=None, kind="lib"):
        folder = folder or self.tmp / "dependencies" / name
        return {"id": (source or "path") + "#" + name, "name": name, "source": source,
                "manifest_path": str(folder / "Cargo.toml"), "dependencies": [],
                "targets": [{"name": target or name, "kind": [kind], "test": True,
                             "src_path": str(folder / "src/lib.rs")}]}

    def local_test_targets(self):
        return [{"name": name, "kind": ["test"], "crate_types": ["bin"], "test": True,
                 "src_path": str(self.root / "elastos/crates/server/tests" / (name + ".rs"))}
                for name in ("integration", "smoke")]

    def push_line(self, ref="refs/heads/fix/fixture", oid=None, remote="refs/heads/fix/fixture"):
        return "{} {} {} {}\n".format(ref, oid or self.git("rev-parse", "HEAD"), remote, "0" * 40)

    def assert_stopped(self, result, message):
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn(message, result.stderr)

    def test_hook_candidate_and_caller_worktree_run_scoped_gates(self):
        result = self.invoke(self.push_line())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        cargo = [c for c in self.commands() if c["tool"] == "cargo"]
        self.assertTrue(any(c["args"] == ["check", "--workspace", "--all-targets"] for c in cargo))
        clippy, = [c for c in cargo if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "server", "--", "-D", "warnings"])
        tests = [c for c in cargo if c["args"][0] == "test"]
        self.assertEqual(len(tests), 2)
        self.assertNotIn("--lib", tests[0]["args"])
        self.assertIn("--bin", tests[0]["args"])
        self.assertIn("--exact", tests[1]["args"])
        self.assertIn("release_cmd::tests::updates", tests[1]["args"])
        self.assertTrue(all(c["cwd"].startswith(str(self.root)) for c in cargo))
        self.assertTrue(all(c["build_dir"] == str(self.root / "target-build") for c in cargo))

    def test_push_refusals_run_before_cargo(self):
        for line in ("", "malformed\n", self.push_line() * 2,
                     self.push_line(oid="f" * 40), self.push_line(ref="refs/heads/other"),
                     self.push_line(remote="refs/tags/candidate"),
                     self.push_line(ref="(delete)", oid="0" * 40)):
            with self.subTest(line=line):
                self.assertNotEqual(self.invoke(line).returncode, 0)
                self.assertEqual(self.commands(), [])

    def test_dirty_source_and_hidden_index_flags_are_refused(self):
        for flag in ("--assume-unchanged", "--skip-worktree"):
            self.git("update-index", flag, "README.md")
            self.assert_stopped(self.invoke(), "index flags")
            self.git("update-index", "--no" + flag[1:], "README.md")
        self.write("untracked.txt", "dirty\n")
        self.assert_stopped(self.invoke(), "working tree changes")
        self.assertEqual(self.commands(), [])

    def test_detached_head_is_refused(self):
        self.git("checkout", "-q", "--detach")
        self.assert_stopped(self.invoke(), "symbolic-ref")
        self.assertEqual(self.commands(), [])

    def test_fetch_failure_and_stale_develop_are_refused(self):
        self.git("remote", "set-url", "origin", str(self.tmp / "missing"))
        self.assert_stopped(self.invoke(), "fetch")
        self.git("remote", "set-url", "origin", str(self.origin))
        self.git("reset", "--hard", "HEAD~1")
        self.git("switch", "-q", "--orphan", "fix/stale")
        self.write("README.md", "stale\n")
        self.commit()
        self.assert_stopped(self.invoke(), "merge current origin/develop")
        self.assertEqual(self.commands(), [])

    def test_changed_source_head_and_remote_base_fail_final_check(self):
        for extra, message in (({"PREPUSH_DIRTY": "1"}, "working tree changes"),
                               ({"PREPUSH_HEAD": "1"}, "HEAD changed during checks"),
                               ({"PREPUSH_ADVANCE": str(self.origin)}, "develop changed during checks")):
            with self.subTest(extra=extra):
                original = self.git("rev-parse", "HEAD")
                self.assert_stopped(self.invoke(extra=extra), message)
                self.git("reset", "--hard", original)

    def advance_develop(self, files):
        self.git("switch", "-q", "develop")
        for path, text in files.items():
            self.write(path, text)
        self.commit()
        self.git("push", "-q", "origin", "develop")
        self.git("switch", "-q", "fix/fixture")

    def test_stale_base_with_clean_disjoint_merge_is_accepted(self):
        self.advance_develop({"README.md": "develop moved\n"})
        candidate = self.git("rev-parse", "HEAD")
        result = self.invoke(self.push_line())
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("base-rule=disjoint-clean-merge", result.stdout)
        self.assertEqual(self.git("rev-parse", "HEAD"), candidate)

    def test_stale_base_with_shared_file_or_conflict_is_refused(self):
        # An identical edit merges cleanly but still overlaps; a file/directory
        # collision conflicts although the changed path names differ.
        for files, branch in (({"elastos/crates/server/src/release_cmd.rs": "// changed module\n",
                                "README.md": "develop moved\n"}, {}),
                              ({"notes": "file\n"}, {"notes/a.md": "directory\n"})):
            with self.subTest(files=files):
                original, develop = self.git("rev-parse", "HEAD"), self.git("rev-parse", "develop")
                self.log.unlink(missing_ok=True)
                for path, text in branch.items():
                    self.write(path, text)
                    self.commit()
                self.advance_develop(files)
                self.assert_stopped(self.invoke(), "merge current origin/develop")
                self.assertEqual(self.commands(), [])
                self.git("reset", "-q", "--hard", original)
                self.git("branch", "-f", "develop", develop)
                self.git("push", "-q", "-f", "origin", "develop")

    def test_zero_and_ignored_tests_fail_and_first_cargo_failure_stops(self):
        for extra, message in (({"PREPUSH_NO_TESTS": "1"}, "zero unit tests"),
                               ({"PREPUSH_ZERO": "1"}, "zero passing unit tests"),
                               ({"PREPUSH_FAIL": "1"}, "cargo check")):
            with self.subTest(extra=extra):
                self.log.unlink(missing_ok=True)
                self.assert_stopped(self.invoke(extra=extra), message)
                if "PREPUSH_FAIL" in extra:
                    self.assertFalse(any(c["args"][0] in {"clippy", "test"} for c in self.commands()))

    def test_empty_touched_binary_cannot_use_library_tests_as_proof(self):
        self.write("elastos/crates/server/src/lib.rs", "// changed library\n")
        self.write("elastos/crates/server/src/main.rs", "mod release_cmd;\n// changed binary\n")
        self.commit()
        result = self.invoke(extra={"PREPUSH_EMPTY_BIN": "1"})
        self.assert_stopped(result, "zero unit tests on this host: --bin server")
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "server", "--lib"] and
                            "--nocapture" in c["args"] for c in self.commands()))

    def test_standalone_binary_and_workspace_manifest_scope(self):
        self.write("capsules/chain-provider/src/main.rs", "// changed binary\n")
        self.write("elastos/Cargo.toml", "[workspace]\n# changed workspace\n")
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        self.assertTrue(any(c["cwd"] == str(self.root / "capsules/chain-provider") and
                            c["args"] == ["check", "--workspace", "--all-targets"] for c in commands))
        self.assertTrue(any(c["args"][:5] == ["test", "-p", "chain-provider", "--bin", "chain-provider"] for c in commands))
        self.assertTrue(any(c["args"][:3] == ["test", "-p", "other"] for c in commands))
        self.assertFalse(any("wallet-provider" in str(c) for c in commands if c["args"][0] != "metadata"))

    def test_runtime_input_checks_direct_and_transitive_path_consumers(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("elastos/crates/common/src/lib.rs", "// changed Runtime common crate\n")
        self.commit()
        graph = {
            "elastos/crates/server": ["elastos/crates/common"],
            "capsules/chain-provider": [{"path": "elastos/crates/server", "name": "alias",
                                         "rename": "runtime_alias", "kind": "build", "optional": True,
                                         "target": 'cfg(target_os = "linux")'}],
            "capsules/wallet-provider": [{"path": "capsules/chain-provider", "kind": "dev"}],
        }
        result = self.invoke(extra={"PREPUSH_GRAPH": json.dumps(graph)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        checks = [c for c in commands if c["args"] == ["check", "--workspace", "--all-targets"]]
        self.assertEqual({c["cwd"] for c in checks}, {str(self.root / "elastos"),
                         str(self.root / "capsules/chain-provider"), str(self.root / "capsules/wallet-provider")})
        clippy, = [c for c in commands if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "common", "--", "-D", "warnings"])
        self.assertTrue(all(c["args"][2] == "common" for c in commands if c["args"][0] == "test"))
        metadata = [c for c in commands if c["args"][0] == "metadata"]
        self.assertEqual(len(metadata), len({tuple(c["args"]) for c in metadata}))

    def test_docs_only_change_keeps_standalone_metadata_discovery_outside_gate(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("README.md", "Changed documentation\n")
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        metadata = [c for c in self.commands() if c["args"][0] == "metadata"]
        self.assertEqual(len(metadata), 2)
        self.assertTrue(all(c["args"][-1] == str(self.root / "elastos/Cargo.toml") for c in metadata))
        self.assertEqual(sum("--no-deps" in c["args"] for c in metadata), 1)

    def test_shared_runtime_input_seeds_all_members(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("elastos/wit/input.wit", "// changed shared contract\n")
        self.commit()
        graph = {"capsules/chain-provider": ["elastos/crates/common"]}
        result = self.invoke(extra={"PREPUSH_GRAPH": json.dumps(graph)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(any(c["cwd"] == str(self.root / "capsules/chain-provider") and c["args"][0] == "check"
                            for c in self.commands()))
        clippy, = [c for c in self.commands() if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "server", "-p", "other",
                                        "-p", "common", "--", "-D", "warnings"])
        self.assertEqual({c["args"][2] for c in self.commands() if c["args"][0] == "test"},
                         {"server", "other", "common"})

    def test_template_data_and_source_select_runtime_consumer_units(self):
        self.git("reset", "--hard", "HEAD~1")
        template = "templates/capsules/component-app/"
        self.write(template + "Cargo.toml", '[package]\nname = "template"\n')
        self.write(template + "capsule.json", '{}\n')
        self.write("elastos/crates/common/src/lib.rs",
                   'const TEMPLATE: &str = include_str!("../../../../templates/capsules/component-app/capsule.json");\n')
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")
        self.write(template + "capsule.json", '{"changed":true}\n')
        self.write(template + "src/lib.rs", "// changed capsule template source\n")
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        self.assertFalse(any(template in str(c) for c in commands))
        clippy, = [c for c in commands if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "common", "--", "-D", "warnings"])
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "common", "--lib"] and "--nocapture" in c["args"]
                            for c in commands))

    def test_external_fixture_data_selects_runtime_consumer_units(self):
        self.git("reset", "--hard", "HEAD~1")
        fixture = "elastos/tests/fixtures/probe/input.json"
        self.write(fixture, '{}\n')
        self.write("elastos/crates/common/src/lib.rs",
                   'const PROBE: &str = include_str!("../../../tests/fixtures/probe/input.json");\n')
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")
        self.write(fixture, '{"changed":true}\n')
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        clippy, = [c for c in self.commands() if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "common", "--", "-D", "warnings"])
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "common", "--lib"] and "--nocapture" in c["args"]
                            for c in self.commands()))

    def test_mixed_known_template_and_unknown_fixture_widen_runtime_units(self):
        self.git("reset", "--hard", "HEAD~1")
        template = "templates/capsules/component-app/capsule.json"
        self.write(template, '{}\n')
        self.write("elastos/crates/common/src/lib.rs",
                   'const TEMPLATE: &str = include_str!("../../../../templates/capsules/component-app/capsule.json");\n')
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")
        self.write(template, '{"changed":true}\n')
        self.write("elastos/tests/fixtures/dynamic/input.json", '{"changed":true}\n')
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        clippy, = [c for c in self.commands() if c["args"][0] == "clippy"]
        self.assertEqual(clippy["args"], ["clippy", "--all-targets", "-p", "server", "-p", "other",
                                        "-p", "common", "--", "-D", "warnings"])
        self.assertEqual({c["args"][2] for c in self.commands() if c["args"][0] == "test"},
                         {"server", "other", "common"})

    def external_baseline(self, source, inputs):
        self.git("reset", "--hard", "HEAD~1")
        self.write("elastos/crates/server/src/consumer.rs", source)
        for path, text in inputs.items():
            self.write(path, text)
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")

    def assert_server_input_units(self):
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        clippy = [c for c in commands if c["args"][0] == "clippy" and c["cwd"] == str(self.root / "elastos")]
        self.assertEqual([c["args"] for c in clippy],
                         [["clippy", "--all-targets", "-p", "server", "--", "-D", "warnings"]])
        tests = [c for c in commands if c["args"][0] == "test" and c["args"][2] == "server"
                 and "--nocapture" in c["args"]]
        self.assertEqual(len(tests), 2)
        self.assertTrue(any("--lib" in c["args"] for c in tests))
        self.assertTrue(any("--bin" in c["args"] for c in tests))

    def test_components_manifest_reads_select_runtime_units(self):
        self.external_baseline('fn read() { let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../");\n'
                               'fs::read(root.join("components.json")).unwrap(); }\n', {"components.json": '{}\n'})
        self.write("components.json", '{"changed":true}\n')
        self.commit()
        self.assert_server_input_units()

    def test_capsule_manifest_reads_select_runtime_and_capsule_units(self):
        capsule = "capsules/chain-provider/capsule.json"
        self.external_baseline('fn read() { let path = Path::new(env!("CARGO_MANIFEST_DIR"))\n'
                               '.join("../../../capsules/chain-provider/capsule.json"); fs::read(path).unwrap(); }\n',
                               {capsule: '{}\n'})
        self.write(capsule, '{"changed":true}\n')
        self.commit()
        self.assert_server_input_units()
        self.assertTrue(any(c["args"][:3] == ["test", "-p", "chain-provider"] and "--nocapture" in c["args"]
                            for c in self.commands()))

    def test_home_browser_directory_reads_select_runtime_units(self):
        asset = "capsules/home/browser/shell-auth.js"
        self.external_baseline('fn read() { let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");\n'
                               'copy_home(&repo.join("capsules/home/browser"), &home); }\n',
                               {asset: 'export const input = false;\n'})
        self.write(asset, 'export const input = true;\n')
        self.commit()
        self.assert_server_input_units()

    def test_runtime_launched_script_selects_runtime_units(self):
        script = "scripts/browser-vm-remote-vz-launcher.integration.mjs"
        self.external_baseline('fn launch() { let script = Path::new(env!("CARGO_MANIFEST_DIR"))\n'
                               '.join("../../../scripts/browser-vm-remote-vz-launcher.integration.mjs");\n'
                               'Command::new("node").arg(script).spawn(); }\n', {script: '// input\n'})
        self.write(script, '// changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_runtime_chained_join_script_name_selects_runtime_units(self):
        script = "scripts/setup-source-home.sh"
        self.external_baseline('fn read() { let script = fs::read_to_string(\n'
                               'Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..")\n'
                               '.join("scripts").join("setup-source-home.sh")).unwrap(); }\n',
                               {script: '# input\n'})
        self.write(script, '# changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_rust_lifetimes_and_character_quotes_preserve_path_literals(self):
        script = "scripts/runtime-owned.sh"
        self.external_baseline("fn read<'a>(source: &'a str) { let delimiter = '\"';\n"
                               'let script = Path::new(env!("CARGO_MANIFEST_DIR"))\n'
                               '.join("../../../scripts/runtime-owned.sh"); fs::read_to_string(script).unwrap(); }\n',
                               {script: '# input\n'})
        self.write(script, '# changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_capsule_tools_helper_dependencies_select_runtime_units(self):
        tool = "capsules/tools/runtime-helper.sh"
        helper = "capsules/tools/input-helper.py"
        self.external_baseline('fn launch() { Command::new("bash")\n'
                               '.arg(repo.join("capsules/tools/runtime-helper.sh")).spawn(); }\n',
                               {tool: 'helper="$(dirname "${BASH_SOURCE[0]}")/input-helper.py"\npython3 "$helper"\n',
                                helper: '# input\n'})
        self.write(helper, '# changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_python_comment_apostrophe_preserves_double_quoted_helper(self):
        tool, helper = "scripts/media-tools-build.py", "scripts/build-media-tools.sh"
        self.external_baseline('fn launch() { Command::new("python3")\n'
                               '.arg(repo.join("scripts/media-tools-build.py")).spawn(); }\n',
                               {tool: '# Match the server\'s archive bytes.\nhelper = "build-media-tools.sh"\n# The operator\'s helper.\n',
                                helper: '# input\n'})
        self.write(helper, '# changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_node_comment_apostrophe_preserves_double_quoted_helper(self):
        tool, helper = "scripts/runtime-owned.mjs", "scripts/node-helper.mjs"
        self.external_baseline('fn launch() { Command::new("node")\n'
                               '.arg(repo.join("scripts/runtime-owned.mjs")).spawn(); }\n',
                               {tool: '// The caller\'s helper.\nnew URL("./node-helper.mjs", import.meta.url);\n// The operator\'s input.\n',
                                helper: '// input\n'})
        self.write(helper, '// changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_deleted_transitive_runtime_script_selects_runtime_units(self):
        wrapper = "scripts/browser-vm-remote-vz-launcher.integration.mjs"
        launcher = "scripts/browser-vm-remote-vz-launcher.mjs"
        self.external_baseline('fn launch() { let script = Path::new(env!("CARGO_MANIFEST_DIR"))\n'
                               '.join("../../../scripts/browser-vm-remote-vz-launcher.integration.mjs");\n'
                               'Command::new("node").arg(script).spawn(); }\n',
                               {wrapper: 'const wrapper = new URL("./browser-vm-remote-vz-launcher.mjs", import.meta.url);\n',
                                launcher: '// input\n'})
        self.git("rm", launcher)
        self.commit()
        self.assert_server_input_units()

    def test_computed_shell_helper_dependency_selects_runtime_units(self):
        script = "scripts/publish-release.sh"
        helper = "scripts/discover-source-bootstrap.py"
        self.external_baseline('fn publish() { Command::new("bash")\n'
                               '.arg(workspace_root.join("scripts/publish-release.sh")).spawn(); }\n',
                               {script: 'helper="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/discover-source-bootstrap.py"\n'
                                        'python3 "$helper"\n', helper: '# input\n'})
        self.write(helper, '# changed input\n')
        self.commit()
        self.assert_server_input_units()

    def test_unreferenced_ci_python_node_tools_keep_their_own_checks(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("elastos/crates/server/src/lib.rs", 'const TOOLS: &str = "scripts";\n')
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")
        for path in ("scripts/ci-example.py", "scripts/ci-example.mjs", "scripts/ci-local-prepush.py"):
            self.write(path, "// owned tooling\n")
        self.commit()
        with mock.patch.dict(os.environ, self.env, clear=True):
            owners = GATE.external_input_owners(self.root,
                                               [{"manifest_path": str(self.root / "elastos/crates/server/Cargo.toml")}],
                                               ["scripts/ci-example.py", "scripts/ci-example.mjs", "scripts/ci-local-prepush.py"])
        self.assertEqual(owners, set())

    def test_broad_runtime_input_refuses_a_selected_target_with_zero_tests(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("elastos/config/input.json", '{}\n')
        self.commit()
        result = self.invoke(extra={"PREPUSH_EMPTY_PACKAGE": "other"})
        self.assert_stopped(result, "other has zero unit tests")
        self.assertTrue(any(c["args"][0] == "clippy" and "other" in c["args"] for c in self.commands()))

    def test_hook_git_environment_cannot_mutate_owned_sentinel_repository(self):
        sentinel = self.tmp / "sentinel"
        sentinel.mkdir()
        def sentinel_git(*args):
            return subprocess.check_output(["git", *args], cwd=sentinel,
                                           env={**self.env, "GIT_OPTIONAL_LOCKS": "0"}, text=True)
        sentinel_git("init", "-q", "-b", "sentinel")
        (sentinel / "keep.txt").write_text("sentinel content\n")
        sentinel_git("add", "keep.txt")
        sentinel_git("commit", "-qm", "owned sentinel")
        (sentinel / "keep.txt").write_text("sentinel dirty content\n")
        (sentinel / "staged.txt").write_text("sentinel staged content\n")
        sentinel_git("add", "staged.txt")
        (sentinel / "untracked.txt").write_text("sentinel untracked content\n")
        def state():
            return (sentinel_git("rev-parse", "HEAD"), sentinel_git("rev-parse", "HEAD^{tree}"),
                    (sentinel / ".git/index").read_bytes(),
                    sentinel_git("status", "--porcelain=v1", "-z", "--untracked-files=all"))
        before = state()
        hostile = {**self.env, "GIT_DIR": str(sentinel / ".git"), "GIT_WORK_TREE": str(sentinel),
                   "GIT_INDEX_FILE": str(sentinel / ".git/index"), "GIT_COMMON_DIR": str(sentinel / ".git"),
                   "GIT_CONFIG_COUNT": "1", "GIT_CONFIG_KEY_0": "core.hooksPath", "GIT_CONFIG_VALUE_0": "/dev/null"}
        child = subprocess.run([sys.executable, str(Path(__file__).resolve()),
                                "PrepushTests.test_docs_only_change_keeps_standalone_metadata_discovery_outside_gate"],
                               cwd=sentinel, env=hostile, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(child.returncode, 0, child.stdout + child.stderr)
        self.assertEqual(before, state(), "foreign fixture Git operations changed the owned sentinel")

    def test_block_comment_module_declarations_widen_unit_scope(self):
        entry = self.root / "elastos/crates/server/src/lib.rs"
        entry.write_text("/*\nmod runtime;\n*/\n")
        changed = self.root / "elastos/crates/server/src/runtime.rs"
        self.assertIsNone(GATE.module_prefix(entry, changed))

    def test_discovery_exclusions_use_only_paths_inside_the_repository(self):
        self.git("reset", "--hard", "HEAD~1")
        self.write("templates/example/Cargo.toml", "// excluded template\n")
        self.write("elastos/tests/fixtures/example/Cargo.toml", "// excluded fixture\n")
        self.commit()
        self.git("push", "-q", "origin", "HEAD:develop")
        self.write("elastos/crates/server/src/release_cmd.rs", "// changed Runtime source\n")
        self.commit()
        moved = self.tmp / "fixtures/source"
        moved.parent.mkdir()
        self.root.rename(moved)
        self.root = moved
        self.env["PREPUSH_ROOT"] = str(moved)
        graph = {"capsules/chain-provider": ["elastos/crates/server"]}
        result = self.invoke(extra={"PREPUSH_GRAPH": json.dumps(graph)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(any(c["cwd"] == str(self.root / "capsules/chain-provider") and c["args"][0] == "check"
                            for c in self.commands()))
        metadata = [c["args"][-1] for c in self.commands() if c["args"][0] == "metadata"]
        self.assertNotIn(str(self.root / "templates/example/Cargo.toml"), metadata)
        self.assertNotIn(str(self.root / "elastos/tests/fixtures/example/Cargo.toml"), metadata)

    def test_rename_paths_with_newlines_and_removed_package_are_preserved(self):
        self.git("mv", "elastos/crates/server/src/release_cmd.rs", "capsules/wallet-provider/src/renamed\nmodule.rs")
        self.commit()
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(any(c["args"][:3] == ["test", "-p", "wallet-provider"] for c in self.commands()))
        (self.root / "capsules/wallet-provider/Cargo.toml").unlink()
        self.commit()
        self.assert_stopped(self.invoke(), "removed Rust package")

    def test_busy_lease_refuses_and_retains_the_same_inode(self):
        lock = self.root / ".git/local-ai-heavy-build.lock"
        with lock.open("a+") as lease:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            inode = lock.stat().st_ino
            self.assert_stopped(self.invoke(), "lease is busy")
            self.assertEqual(self.commands(), [])
        self.assertEqual(self.invoke().returncode, 0)
        self.assertEqual(lock.stat().st_ino, inode)

    def test_explicit_shared_build_dir_is_preserved(self):
        cache = str(self.tmp / "shared")
        self.assertEqual(self.invoke(extra={"CARGO_BUILD_BUILD_DIR": cache}).returncode, 0)
        self.assertTrue(all(c["build_dir"] == cache for c in self.commands()))

    def test_relative_shared_build_dir_is_normalized_for_all_workspaces(self):
        self.write("capsules/chain-provider/src/main.rs", "// changed\n")
        self.commit()
        self.assertEqual(self.invoke(extra={"CARGO_BUILD_BUILD_DIR": "../shared"}).returncode, 0)
        self.assertTrue(all(c["build_dir"] == str(self.tmp / "shared") for c in self.commands()))

    def test_repository_clean_runs_only_for_a_shared_build_dir(self):
        for extra, cleaned in (({}, True),
                               ({"CARGO_BUILD_BUILD_DIR": str(self.tmp / "shared")}, True),
                               ({"CARGO_BUILD_BUILD_DIR": str(self.root / "target/private")}, False),
                               ({"CARGO_BUILD_BUILD_DIR": str(self.tmp / "shared"),
                                 "ELASTOS_PREPUSH_PRIVATE_BUILD_DIR": "1"}, False)):
            with self.subTest(extra=extra):
                self.log.unlink(missing_ok=True)
                result = self.invoke(extra=extra)
                self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
                cargo = [c["args"][0] for c in self.commands() if c["tool"] == "cargo"]
                self.assertIn("check", cargo)
                self.assertEqual("clean" in cargo, cleaned)

    def test_repository_clean_covers_all_members_and_forward_path_dependencies(self):
        self.write("capsules/chain-provider/src/main.rs", "// changed\n")
        self.commit()
        graph = {"elastos/crates/server": ["capsules/custody-provider"],
                 "capsules/custody-provider": ["capsules/chain-provider"],
                 "capsules/chain-provider": ["elastos/crates/common"]}
        # Cargo strips the hash before matching fingerprint/build package
        # names, and normalizes the compiled crate name to server_extension.
        foreign = [self.dependency_package("server-extension", "registry+fixture", target="server-extension"),
                   self.dependency_package("git-library", "git+fixture"),
                   self.dependency_package("external-library", None),
                   self.dependency_package("registry-library", "registry+fixture", self.root / "vendor/registry-library")]
        result = self.invoke(extra={"PREPUSH_GRAPH": json.dumps(graph),
                                    "PREPUSH_DEPENDENCY_PACKAGES": json.dumps(foreign)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        cleans = [c for c in commands if c["args"][0] == "clean"]
        self.assertEqual(len(cleans), 2)
        by_workspace = {c["cwd"]: c["args"] for c in cleans}
        self.assertEqual(by_workspace[str(self.root / "elastos")],
                         ["clean", "--locked", "--offline", "-p", "chain-provider", "-p", "common",
                          "-p", "custody-provider", "-p", "other", "-p", "server"])
        self.assertEqual(by_workspace[str(self.root / "capsules/chain-provider")],
                         ["clean", "--locked", "--offline", "-p", "chain-provider", "-p", "common"])
        first_build = next(index for index, c in enumerate(commands)
                           if c["args"][0] in {"check", "clippy", "test", "build"})
        self.assertTrue(all(commands.index(c) < first_build for c in cleans))
        resolved = [c for c in commands if c["args"][0] == "metadata" and "--no-deps" not in c["args"]]
        self.assertEqual(len(resolved), 2)
        self.assertTrue(all("--locked" in c["args"] and "--offline" not in c["args"] for c in resolved))
        self.assertTrue(all(c["args"][-1] == str(Path(c["cwd"]) / "Cargo.toml") for c in resolved))

    def test_registry_and_git_package_name_collisions_refuse_before_clean(self):
        for source, kind in (("registry+fixture", "lib"), ("git+fixture", "lib"),
                             ("registry+fixture", "custom-build")):
            with self.subTest(source=source, kind=kind):
                self.log.unlink(missing_ok=True)
                foreign = self.dependency_package("server", source, target="foreign_target", kind=kind)
                result = self.invoke(extra={"PREPUSH_DEPENDENCY_PACKAGES": json.dumps([foreign])})
                self.assert_stopped(result, "clean collides with external artifact names: server")
                self.assertFalse(any(c["args"][0] in {"clean", "check", "clippy", "test", "build"}
                                     for c in self.commands()))

    def test_foreign_normalized_target_collisions_refuse_before_clean(self):
        for source in ("registry+fixture", None):
            with self.subTest(source=source):
                self.log.unlink(missing_ok=True)
                foreign = self.dependency_package("foreign-library", source, target="server_probe")
                result = self.invoke(extra={"PREPUSH_PACKAGE": "server-probe",
                                            "PREPUSH_DEPENDENCY_PACKAGES": json.dumps([foreign])})
                self.assert_stopped(result, "clean collides with external artifact names: server_probe")
                self.assertFalse(any(c["args"][0] in {"clean", "check", "clippy", "test", "build"}
                                     for c in self.commands()))

    def test_inactive_foreign_targets_do_not_collide_with_local_test_names(self):
        # Registry metadata includes a distinct dependency library as well as
        # integration/smoke test executables with crate_types=[bin]. The target
        # kind, not crate_types, distinguishes those inactive package targets.
        foreign = []
        for kind in ("test", "example", "bench", "bin"):
            package = self.dependency_package("foreign-" + kind, "registry+fixture")
            package["targets"].extend(
                {"name": name, "kind": [kind], "crate_types": ["bin"], "test": True,
                 "src_path": str(self.tmp / "dependencies" / ("foreign-" + kind) / (name + ".rs"))}
                for name in ("integration", "smoke"))
            foreign.append(package)
        result = self.invoke(extra={"PREPUSH_LOCAL_TARGETS": json.dumps(self.local_test_targets()),
                                    "PREPUSH_DEPENDENCY_PACKAGES": json.dumps(foreign)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        clean, = [c for c in self.commands() if c["args"][0] == "clean"]
        self.assertEqual(clean["args"], ["clean", "--locked", "--offline", "-p", "common", "-p", "other", "-p", "server"])
        self.assertTrue(any(c["args"][0] == "check" for c in self.commands()))

    def test_active_foreign_library_collides_with_local_test_names(self):
        for kind, name in (("lib", "integration"), ("proc-macro", "smoke")):
            with self.subTest(kind=kind, name=name):
                self.log.unlink(missing_ok=True)
                foreign = self.dependency_package("foreign-library", "registry+fixture", target=name, kind=kind)
                foreign["targets"][0]["crate_types"] = [kind]
                result = self.invoke(extra={"PREPUSH_LOCAL_TARGETS": json.dumps(self.local_test_targets()),
                                            "PREPUSH_DEPENDENCY_PACKAGES": json.dumps([foreign])})
                self.assert_stopped(result, "clean collides with external artifact names: " + name)
                self.assertFalse(any(c["args"][0] in {"clean", "check", "clippy", "test", "build"}
                                     for c in self.commands()))

    def test_foreign_custom_build_target_does_not_collide_with_local_target(self):
        foreign = self.dependency_package("foreign-library", "registry+fixture", target="server", kind="custom-build")
        result = self.invoke(extra={"PREPUSH_DEPENDENCY_PACKAGES": json.dumps([foreign])})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        clean, = [c for c in self.commands() if c["args"][0] == "clean"]
        self.assertEqual(clean["args"], ["clean", "--locked", "--offline", "-p", "common", "-p", "other", "-p", "server"])

    def test_empty_repository_clean_scope_refuses_before_clean_command(self):
        foreign = self.dependency_package("external-library", None)
        with mock.patch.object(GATE, "run", side_effect=["elastos/Cargo.lock\0", json.dumps({"packages": [foreign]})]) as run:
            with self.assertRaisesRegex(GATE.GateError, "empty scope"):
                GATE.clean_repository_packages(self.root, self.root / "elastos", None)
        self.assertEqual(run.call_count, 2)
        self.assertEqual(run.call_args.args[0][1], "metadata")

    def test_missing_ignored_workspace_lock_is_generated_and_receipted(self):
        relative = "capsules/chain-provider/Cargo.lock"
        self.git("rm", "-q", relative)
        self.write(".gitignore", (self.root / ".gitignore").read_text() + relative + "\n")
        self.write("capsules/chain-provider/src/main.rs", "// changed\n")
        self.commit()
        candidate = self.git("rev-parse", "HEAD")
        result = self.invoke()
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        lock = self.root / relative
        self.assertTrue(lock.exists())
        self.assertIn("cargo-lock=" + relative + " sha256=" + hashlib.sha256(lock.read_bytes()).hexdigest() +
                      " policy=ignored-generated", result.stdout)
        metadata, = [c for c in self.commands() if c["args"][0] == "metadata" and
                     "--no-deps" not in c["args"] and c["cwd"] == str(lock.parent)]
        self.assertNotIn("--locked", metadata["args"])
        self.assertNotIn("--offline", metadata["args"])
        clean, = [c for c in self.commands() if c["args"][0] == "clean" and c["cwd"] == str(lock.parent)]
        self.assertIn("--locked", clean["args"])
        self.assertIn("--offline", clean["args"])
        self.assertEqual(self.git("status", "--porcelain=v1", "--untracked-files=all"), "")
        self.assertEqual(self.git("rev-parse", "HEAD"), candidate)

    def test_existing_ignored_workspace_lock_updates_with_its_manifest(self):
        relative = "capsules/chain-provider/Cargo.lock"
        lock = self.root / relative
        old = lock.read_bytes()
        self.git("rm", "-q", relative)
        self.write(".gitignore", (self.root / ".gitignore").read_text() + relative + "\n")
        self.write("capsules/chain-provider/Cargo.toml", '[package]\nname = "chain-provider"\n# dependency changed\n')
        self.commit()
        lock.write_bytes(old)
        result = self.invoke(extra={"PREPUSH_UPDATE_LOCK": "capsules/chain-provider"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertNotEqual(lock.read_bytes(), old)
        self.assertIn("cargo-lock=" + relative + " sha256=" + hashlib.sha256(lock.read_bytes()).hexdigest() +
                      " policy=ignored-generated", result.stdout)
        metadata, = [c for c in self.commands() if c["args"][0] == "metadata" and
                     "--no-deps" not in c["args"] and c["cwd"] == str(lock.parent)]
        self.assertNotIn("--locked", metadata["args"])
        self.assertEqual(self.git("status", "--porcelain=v1", "--untracked-files=all"), "")

    def test_cold_metadata_fetch_preserves_the_committed_lock(self):
        lock = self.root / "elastos/Cargo.lock"
        old = lock.read_bytes()
        result = self.invoke(extra={"PREPUSH_COLD_METADATA": "elastos"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        metadata, = [c for c in self.commands() if c["args"][0] == "metadata" and "--no-deps" not in c["args"]]
        self.assertIn("--locked", metadata["args"])
        self.assertNotIn("--offline", metadata["args"])
        self.assertEqual(lock.read_bytes(), old)
        self.assertIn("cargo-lock=elastos/Cargo.lock sha256=" + hashlib.sha256(old).hexdigest() + " policy=tracked",
                      result.stdout)

    def test_committed_lock_update_refuses_before_cleaning(self):
        lock = self.root / "elastos/Cargo.lock"
        old = lock.read_bytes()
        result = self.invoke(extra={"PREPUSH_UPDATE_LOCK": "elastos"})
        self.assert_stopped(result, "command failed: cargo metadata --locked")
        self.assertIn("workspace lock needs an update but --locked was supplied", result.stderr)
        self.assertEqual(lock.read_bytes(), old)
        self.assertFalse(any(c["args"][0] in {"clean", "check", "clippy", "test", "build"} for c in self.commands()))

    def test_missing_unignored_workspace_lock_refuses_before_resolution(self):
        relative = "capsules/chain-provider/Cargo.lock"
        self.git("rm", "-q", relative)
        self.write("capsules/chain-provider/src/main.rs", "// changed\n")
        self.commit()
        result = self.invoke()
        self.assert_stopped(result, "commit or ignore the workspace lockfile before resolution: " + relative)
        self.assertFalse((self.root / relative).exists())
        self.assertFalse(any(c["args"][0] == "metadata" and "--no-deps" not in c["args"] and
                             c["cwd"] == str((self.root / relative).parent) for c in self.commands()))
        self.assertEqual(self.git("status", "--porcelain=v1", "--untracked-files=all"), "")

    def test_inherited_output_directories_are_overridden_for_each_workspace(self):
        self.write("capsules/chain-provider/src/main.rs", "// changed\n")
        self.commit()
        shared = str(self.tmp / "shared-intermediates")
        result = self.invoke(extra={"CARGO_BUILD_BUILD_DIR": shared,
                                    "CARGO_TARGET_DIR": str(self.tmp / "global-output"),
                                    "CARGO_BUILD_TARGET_DIR": str(self.tmp / "other-global-output")})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        cargo = [c for c in self.commands() if c["tool"] == "cargo"]
        self.assertGreaterEqual(len({c["cwd"] for c in cargo}), 2)
        for command in cargo:
            self.assertEqual(command["target_dir"], str(Path(command["cwd"]) / "target"))
            self.assertEqual(command["build_target_dir"], command["target_dir"])
            self.assertEqual(command["build_dir"], shared)

    def test_cdylib_units_and_unknown_module_fallback(self):
        self.write("capsules/chat-room-ui/src/lib.rs", "// changed\n")
        self.write("elastos/crates/server/src/helpers.rs", "// ambiguous test owner\n")
        self.commit()
        self.assertEqual(self.invoke().returncode, 0)
        tests = [c for c in self.commands() if c["args"][0] == "test"]
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "chat-room-ui", "--lib"] for c in tests))
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "server", "--lib"] for c in tests))
        self.assertTrue(any(c["args"][:4] == ["test", "-p", "server", "--bin"] for c in tests))

    def test_full_server_units_prepare_candidate_process_providers(self):
        self.write("elastos/crates/server/src/lib.rs", "// changed crate root\n")
        self.commit()
        providers = ("protected-content-protect-provider", "protected-content-decrypt-provider", "custody-provider")
        graph = {"capsules/" + name: ["elastos/crates/common"] for name in providers}
        graph["elastos/crates/common"] = ["elastos/crates/other"]
        result = self.invoke(extra={"PREPUSH_PACKAGE": "elastos-server",
                                    "PREPUSH_GRAPH": json.dumps(graph),
                                    "PREPUSH_TESTS": "protected_content_runtime::tests::process: test",
                                    "ELASTOS_TEST_PROTECT_PROVIDER_BIN": str(self.tmp / "stale"),
                                    "ELASTOS_TEST_UNRELATED_BIN": str(self.tmp / "ambient")})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        commands = self.commands()
        builds = [c for c in commands if c["args"][0] == "build"]
        self.assertEqual(len(builds), 3)
        self.assertTrue(all(c["args"] == ["build", "--release", "--target-dir", "target"] for c in builds))
        cleans = [c for c in commands if c["args"][0] == "clean" and "--release" in c["args"]]
        self.assertEqual({c["cwd"] for c in cleans}, {str(self.root / "capsules" / name) for name in providers})
        self.assertEqual(len(cleans), 3)
        self.assertTrue(all(commands.index(c) < commands.index(builds[0]) for c in cleans))
        for clean in cleans:
            expected = ["clean", "--locked", "--offline", "--release"]
            for name in sorted({Path(clean["cwd"]).name, "common", "other"}):
                expected.extend(["-p", name])
            self.assertEqual(clean["args"], expected)
        tests = [c for c in commands if c["args"][0] == "test" and "--list" not in c["args"]]
        self.assertTrue(all(len(c["provider_env"]) == 3 for c in tests))
        self.assertTrue(all(str(self.root / "capsules") in path for c in tests for path in c["provider_env"].values()))

    def test_ambient_test_binary_paths_are_removed_before_product_checks(self):
        result = self.invoke(extra={"ELASTOS_TEST_PROTECT_PROVIDER_BIN": str(self.tmp / "stale"),
                                    "ELASTOS_TEST_MEDIA_BIN": str(self.tmp / "ambient")})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertTrue(all(not c["provider_env"] for c in self.commands()))

    def test_product_child_git_operations_preserve_owned_caller_sentinel(self):
        nested = self.tmp / "nested-git"
        nested.mkdir()
        def nested_git(*args):
            return subprocess.check_output(["git", *args], cwd=nested, env=self.env, text=True).strip()
        nested_git("init", "-q", "-b", "owned-product-fixture")
        (nested / "owned.txt").write_text("owned disposable child repository\n")
        nested_git("add", "owned.txt")
        nested_git("commit", "-qm", "owned sentinel")
        old_nested = nested_git("rev-parse", "HEAD")
        def caller_state():
            return (self.git("rev-parse", "HEAD"), self.git("rev-parse", "HEAD^{tree}"),
                    (self.root / ".git/index").read_bytes(),
                    self.git("status", "--porcelain=v1", "-z", "--untracked-files=all"))
        before = caller_state()
        local_names = set(self.git("rev-parse", "--local-env-vars").splitlines())
        result = self.invoke(extra={"GIT_DIR": str(self.root / ".git"), "GIT_WORK_TREE": str(self.root),
                                    "GIT_INDEX_FILE": str(self.root / ".git/index"), "GIT_COMMON_DIR": str(self.root / ".git"),
                                    "PREPUSH_PRODUCT_SENTINEL": str(nested)})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(before, caller_state(), "product child Git selected the caller repository")
        self.assertNotEqual(old_nested, nested_git("rev-parse", "HEAD"))
        self.assertTrue(all(not local_names.intersection(c["git_env"]) for c in self.commands()))

    def test_module_names_use_exact_filters_with_overlapping_names(self):
        self.write("elastos/crates/server/src/runtime.rs", "// changed runtime module\n")
        self.commit()
        result = self.invoke(extra={"PREPUSH_PACKAGE": "elastos-server",
                                    "PREPUSH_LIB_TESTS": "runtime::tests::local: test\nprotected_content_runtime::tests::process: test"})
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        tests = [c for c in self.commands() if c["args"][0] == "test" and "--list" not in c["args"]]
        library, = [c for c in tests if "--lib" in c["args"]]
        self.assertIn("--exact", library["args"])
        self.assertIn("runtime::tests::local", library["args"])
        self.assertNotIn("protected_content_runtime::tests::process", library["args"])
        self.assertFalse(any(c["args"][0] == "build" for c in self.commands()))

    def test_committed_hook_calls_reviewed_source_for_caller_candidate(self):
        hook = SCRIPT.parent.parent / ".githooks/pre-push"
        result = subprocess.run([str(hook), "origin", str(self.origin)], cwd=self.root, env=self.env,
                                input=self.push_line(), text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_disk_reserve_refuses_before_builds(self):
        with mock.patch.object(GATE.shutil, "disk_usage", return_value=shutil_usage(100, 86, 14)):
            with self.assertRaisesRegex(GATE.GateError, "15% disk reserve"):
                GATE.disk_reserve(self.root)
        with mock.patch.object(GATE.shutil, "disk_usage", return_value=shutil_usage(100, 85, 15)):
            GATE.disk_reserve(self.root)

    def test_interrupt_settles_owned_cargo_and_releases_lease(self):
        child_file = self.tmp / "child.pid"
        process = subprocess.Popen([str(SCRIPT)], cwd=self.root,
                                   env={**self.env, "PREPUSH_SLEEP": "1", "PREPUSH_CHILD": str(child_file)},
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not child_file.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(child_file.exists(), "Cargo descendant fixture did not start")
            child = int(child_file.read_text())
            process.send_signal(signal.SIGTERM)
            _, error = process.communicate(timeout=7)
            self.assertNotEqual(process.returncode, 0)
            self.assertIn("received signal", error)
            status = subprocess.run(["ps", "-p", str(child), "-o", "stat="], text=True, stdout=subprocess.PIPE)
            self.assertTrue(status.returncode != 0 or status.stdout.strip().startswith("Z"),
                            "owned descendant survived interruption: " + status.stdout)
            with (self.root / ".git/local-ai-heavy-build.lock").open("a+") as lease:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()

    def test_signal_at_child_launch_settles_owned_group_before_lease_release(self):
        child_file = self.tmp / "launch.pid"
        lease_file = self.root / ".git/local-ai-heavy-build.lock"
        program = textwrap.dedent('''\
            import fcntl, importlib.util, os, pathlib, signal, subprocess, sys
            spec = importlib.util.spec_from_file_location("gate", sys.argv[1])
            gate = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(gate)
            for signum in gate.INTERRUPTS:
                signal.signal(signum, gate.interrupted)
            original = subprocess.Popen
            def at_launch(*args, **kwargs):
                process = original(*args, **kwargs)
                pathlib.Path(sys.argv[2]).write_text(str(process.pid))
                os.kill(os.getpid(), signal.SIGTERM)
                return process
            gate.subprocess.Popen = at_launch
            with pathlib.Path(sys.argv[3]).open("a+") as lease:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
                gate.run([sys.executable, "-c", "import time; time.sleep(30)"],
                         pathlib.Path.cwd(), lease=lease)
            ''')
        result = subprocess.run([sys.executable, "-c", program, str(SCRIPT.with_suffix(".py")),
                                 str(child_file), str(lease_file)], cwd=self.root, env=self.env,
                                text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=8)
        self.assertNotEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("received signal", result.stderr)
        self.assertTrue(child_file.exists(), "owned child was never launched")
        status = subprocess.run(["ps", "-p", child_file.read_text(), "-o", "stat="],
                                text=True, stdout=subprocess.PIPE)
        self.assertTrue(status.returncode != 0 or status.stdout.strip().startswith("Z"),
                        "child survived a signal at launch: " + status.stdout)
        with lease_file.open("a+") as lease:
            fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)

    def test_second_interrupt_waits_until_term_resistant_group_is_settled(self):
        child_file, cargo_file = self.tmp / "child.pid", self.tmp / "cargo.pid"
        process = subprocess.Popen([str(SCRIPT)], cwd=self.root,
                                   env={**self.env, "PREPUSH_SLEEP": "1", "PREPUSH_CHILD": str(child_file),
                                        "PREPUSH_IGNORE_TERM": str(cargo_file)},
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not child_file.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(child_file.exists(), "TERM-resistant fixture did not start")
            process.send_signal(signal.SIGTERM)
            time.sleep(0.2)
            process.send_signal(signal.SIGINT)
            _, error = process.communicate(timeout=8)
            self.assertNotEqual(process.returncode, 0)
            self.assertIn("pre-push stopped", error)
            for pid_file in (cargo_file, child_file):
                status = subprocess.run(["ps", "-p", pid_file.read_text(), "-o", "stat="],
                                        text=True, stdout=subprocess.PIPE)
                self.assertTrue(status.returncode != 0 or status.stdout.strip().startswith("Z"),
                                "owned process survived repeated interruption: " + status.stdout)
            with (self.root / ".git/local-ai-heavy-build.lock").open("a+") as lease:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            if process.poll() is None:
                if cargo_file.exists():
                    try:
                        os.killpg(int(cargo_file.read_text()), signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                process.kill()
                process.communicate()

    def test_detached_descendant_keeps_no_stable_lease_after_gate_interrupt(self):
        child_file = self.tmp / "detached.pid"
        process = subprocess.Popen([str(SCRIPT)], cwd=self.root,
                                   env={**self.env, "PREPUSH_SLEEP": "1", "PREPUSH_DETACHED": str(child_file)},
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 5
            while not child_file.exists() and time.monotonic() < deadline:
                time.sleep(0.02)
            self.assertTrue(child_file.exists(), "detached fixture did not start")
            with (self.root / ".git/local-ai-heavy-build.lock").open("a+") as lease:
                with self.assertRaises(BlockingIOError):
                    fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
            process.send_signal(signal.SIGTERM)
            _, error = process.communicate(timeout=7)
            self.assertNotEqual(process.returncode, 0)
            self.assertIn("received signal", error)
            child = int(child_file.read_text())
            os.kill(child, 0)  # The deliberately detached fixture still runs.
            with (self.root / ".git/local-ai-heavy-build.lock").open("a+") as lease:
                fcntl.flock(lease, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            if child_file.exists():
                try:
                    os.killpg(int(child_file.read_text()), signal.SIGKILL)
                except ProcessLookupError:
                    pass
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                process.communicate(timeout=7)


def shutil_usage(total, used, free):
    from collections import namedtuple
    return namedtuple("Usage", "total used free")(total, used, free)


if __name__ == "__main__":
    unittest.main()
