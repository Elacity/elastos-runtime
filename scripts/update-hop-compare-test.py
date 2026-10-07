#!/usr/bin/env python3
"""Observer regressions with mocked processes and transport; no Runtime Homes."""

import contextlib
import base64
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
import subprocess
import shutil
from types import SimpleNamespace
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("observer", Path(__file__).with_name("update-hop-compare.py"))
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)
signing_spec = importlib.util.spec_from_file_location("installer_fixture", Path(__file__).with_name("install-bootstrap-test.py"))
signing_fixture = importlib.util.module_from_spec(signing_spec)
signing_spec.loader.exec_module(signing_fixture)


HOLDER_NODE = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"


def public_did(key):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    number, encoded = int.from_bytes(b"\xed\x01" + key, "big"), ""
    while number:
        number, digit = divmod(number, 58)
        encoded = alphabet[digit] + encoded
    return "did:key:z" + encoded


HOLDER_DID = public_did(bytes.fromhex(HOLDER_NODE))


def public_ticket(node=HOLDER_NODE):
    document = {"topic": None, "endpoints": [{"id": node, "addrs": [{"Ip": "127.0.0.1:4433"}]}]}
    return base64.b32encode(json.dumps(document).encode()).decode().lower().rstrip("=")


class CiCapacityTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parent)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.apps = self.root / "Applications"
        self.apps.mkdir()
        for name in observer.CI_XCODE_APPS:
            (self.apps / name).mkdir()
        self.active = self.apps / "Xcode_15.4.app"
        self.override = self.apps / "Xcode_16.2.app"

    def test_reclaim_preserves_both_selected_apps_and_measures_each_deletion(self):
        removed = []
        def remove(path):
            removed.append(path.name)
            path.rmdir()
        def measure():
            return SimpleNamespace(total=1000, free=100 + 100 * len(removed))
        # Reclaim stops as soon as the planned growth fits the free bytes.
        receipt = observer.cli_reclaim_xcode(self.apps, {self.active, self.override}, 300, measure, remove)
        self.assertEqual(removed, ["Xcode_15.0.1.app", "Xcode_15.1.app"])
        self.assertEqual(receipt["status"], "ready")
        self.assertEqual(receipt["free_bytes_after"], 300)
        self.assertEqual(receipt["planned_growth_bytes"], 300)
        self.assertTrue(self.active.is_dir() and self.override.is_dir())
        self.assertTrue((self.apps / "Xcode_15.2.app").exists())

    def test_alias_and_unlisted_apps_are_preserved_and_shortfall_fails_closed(self):
        candidate = self.apps / "Xcode_15.0.1.app"
        candidate.rmdir()
        candidate.symlink_to(self.active, target_is_directory=True)
        (self.apps / "Xcode_99.app").mkdir()
        removed = []
        def remove(path):
            removed.append(path.name)
            path.rmdir()
        receipt = observer.cli_reclaim_xcode(self.apps, {self.active, self.override}, 101,
                                             lambda: SimpleNamespace(total=1000, free=100), remove)
        self.assertEqual(receipt["status"], "unavailable")
        self.assertEqual(receipt["free_bytes_before"], 100)
        self.assertEqual(receipt["free_bytes_after"], 100)
        self.assertEqual([entry["app"] for entry in receipt["removed"]], removed)
        self.assertNotIn(candidate.name, removed)
        self.assertTrue(candidate.is_symlink() and (self.apps / "Xcode_99.app").exists())

    def test_reclaim_refuses_unexpected_selected_ancestry_and_symlink_roots(self):
        outside = self.root / "Xcode_15.4.app"
        outside.mkdir()
        measure, remove = lambda: SimpleNamespace(total=1000, free=100), lambda path: path.rmdir()
        with self.assertRaisesRegex(ValueError, "selected Xcode ancestry"):
            observer.cli_reclaim_xcode(self.apps, {outside}, 100, measure, remove)
        alias = self.root / "alias"
        alias.symlink_to(self.apps, target_is_directory=True)
        with self.assertRaisesRegex(ValueError, "physical Xcode"):
            observer.cli_reclaim_xcode(alias, {self.active}, 100, measure, remove)

    def test_public_reclaim_refuses_local_execution_and_path_override(self):
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted"}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.pwd, "getpwuid", return_value=SimpleNamespace(pw_name="anders")), \
             patch.object(observer.subprocess, "run") as process, self.assertRaisesRegex(ValueError, "disposable hosted Mac"):
            observer.cli_prepare_ci_disk()
        process.assert_not_called()
        with patch.object(observer.sys, "argv", ["observer", "prepare-ci-disk", str(self.apps)]), \
             patch.object(observer, "cli_prepare_ci_disk") as prepare, contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(observer.main(), 2)
        prepare.assert_not_called()

    def android(self):
        runner_home = self.root / "runner"
        sdk = runner_home / "Library/Android/sdk"
        sdk.mkdir(parents=True)
        (sdk / "unused-tool").write_bytes(b"unused CI Android tool")
        (sdk.parent / "keep-user-data").write_bytes(b"keep parent")
        (sdk.parent / "sdk-old").mkdir()
        return runner_home, sdk

    def receipt(self):
        return {"status": "unavailable", "retained": [self.active.name, self.override.name], "removed": [],
                "free_bytes_before": 100, "free_bytes_after": 100, "total_bytes": 1000, "planned_growth_bytes": 101}

    def test_android_reclaim_uses_exact_sdk_and_preserves_apple_tools_and_siblings(self):
        runner_home, sdk = self.android()
        removed = []
        def remove(path):
            self.assertEqual(path, sdk)
            removed.append(path)
            shutil.rmtree(path)
            return 300
        def measure():
            return SimpleNamespace(total=1000, free=100 if not removed else 400)
        receipt = observer.cli_reclaim_android(runner_home, observer.os.geteuid(), self.receipt(), measure, remove)
        self.assertEqual(receipt["status"], "ready")
        self.assertEqual(receipt["removed"], [{"tool": "runner Android SDK", "allocated_bytes": 300, "free_bytes_before": 100, "free_bytes_after": 400}])
        self.assertTrue(self.active.is_dir() and self.override.is_dir())
        self.assertTrue((sdk.parent / "keep-user-data").is_file() and (sdk.parent / "sdk-old").is_dir())

    def test_android_already_ready_needs_no_delete_or_ancestry_probe(self):
        receipt = self.receipt()
        receipt["status"] = "ready"
        with patch.object(observer.Path, "stat", side_effect=AssertionError("unexpected path probe")):
            result = observer.cli_reclaim_android(self.root / "absent", -1, receipt,
                                                  lambda: self.fail("unexpected measure"), lambda _: self.fail("unexpected delete"))
        self.assertIs(result, receipt)

    def test_android_symlink_parent_and_foreign_sdk_owner_are_refused(self):
        runner_home, sdk = self.android()
        for kind in ("foreign owner", "SDK symlink", "parent symlink"):
            with self.subTest(kind=kind), contextlib.ExitStack() as stack:
                if kind == "foreign owner":
                    real_stat = Path.stat
                    def foreign(path, *args, **kwargs):
                        actual = real_stat(path, *args, **kwargs)
                        return SimpleNamespace(st_uid=actual.st_uid + 1, st_mode=actual.st_mode) if path == sdk else actual
                    stack.enter_context(patch.object(observer.Path, "stat", foreign))
                else:
                    selected = sdk if kind == "SDK symlink" else sdk.parent
                    target = selected.with_name(selected.name + "-moved")
                    selected.rename(target)
                    selected.symlink_to(target, target_is_directory=True)
                    stack.callback(lambda path=selected, saved=target: (path.unlink(), saved.rename(path)))
                with self.assertRaisesRegex(ValueError, "Android SDK ancestry or owner"):
                    observer.cli_reclaim_android(runner_home, observer.os.geteuid(), self.receipt(),
                                                lambda: SimpleNamespace(total=1000, free=100), lambda _: self.fail("unexpected delete"))

    def test_android_runner_home_owner_and_nonhosted_execution_are_refused(self):
        runner_home, sdk = self.android()
        with self.assertRaisesRegex(ValueError, "runner home ancestry or owner"):
            observer.cli_reclaim_android(runner_home, observer.os.geteuid() + 1, self.receipt(),
                                        lambda: SimpleNamespace(total=1000, free=100), lambda _: self.fail("unexpected delete"))
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "self-hosted"}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.pwd, "getpwuid", return_value=SimpleNamespace(pw_name="runner")), \
             patch.object(observer.subprocess, "run") as process, self.assertRaisesRegex(ValueError, "disposable hosted Mac"):
            observer.cli_prepare_ci_disk()
        process.assert_not_called()

    def test_android_shortfall_keeps_the_measured_unavailable_receipt(self):
        runner_home, sdk = self.android()
        receipt = observer.cli_reclaim_android(runner_home, observer.os.geteuid(), self.receipt(),
                                               lambda: SimpleNamespace(total=1000, free=100), shutil.rmtree)
        self.assertEqual(receipt["status"], "unavailable")
        self.assertEqual(receipt["free_bytes_after"], 100)
        self.assertEqual(len(receipt["removed"]), 1)
        error = ValueError("hosted Mac capacity unavailable")
        error.capacity = receipt
        stderr = io.StringIO()
        with patch.object(observer.sys, "argv", ["observer", "prepare-ci-disk"]), \
             patch.object(observer, "cli_prepare_ci_disk", side_effect=error), contextlib.redirect_stderr(stderr):
            self.assertEqual(observer.main(), 2)
        self.assertEqual(json.loads(stderr.getvalue())["capacity"], receipt)


class HolderTransportTests(unittest.TestCase):
    def test_canonical_public_did_derives_carrier_node_bytes(self):
        self.assertEqual(observer.cli_holder_node_id(HOLDER_DID), HOLDER_NODE)
        # A second public RFC 8032 vector exercises the other compressed sign bit.
        node = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c"
        self.assertEqual(observer.cli_holder_node_id(public_did(bytes.fromhex(node))), node)

    def test_canonical_did_refuses_aliases_wrong_codec_and_malformed_encoding(self):
        values = [None, {}, "did:key:holder", HOLDER_DID + "1", HOLDER_DID.replace("z6Mk", "z6Mm"),
                  HOLDER_DID[:-1] + "0", "did:key:z1" + HOLDER_DID[9:],
                  public_did(bytes.fromhex(HOLDER_NODE)[:-1]), public_did(bytes.fromhex(HOLDER_NODE) + b"\0")]
        for value in values:
            with self.subTest(value=value), self.assertRaisesRegex(ValueError, "canonical Ed25519"):
                observer.cli_holder_node_id(value)

    def test_public_bootstrap_shape_has_no_did_and_binds_owned_node(self):
        public = {"schema": "elastos.carrier.bootstrap/v1", "transport": "carrier", "role": "publisher",
                  "node_id": HOLDER_NODE, "ticket": public_ticket(), "generated_at": 1}
        observer.cli_holder_bootstrap(public, HOLDER_NODE)
        for key, value in [("schema", None), ("schema", "other"), ("transport", "other"), ("role", "runtime"),
                           ("node_id", None), ("node_id", 123), ("node_id", "f" * 64), ("node_id", HOLDER_NODE.upper()),
                           ("ticket", None), ("ticket", 123), ("ticket", ""), ("ticket", "invalid-ticket"),
                           ("ticket", "a"), ("ticket", "a" * 65537)]:
            with self.subTest(key=key, value=value), self.assertRaises(ValueError):
                observer.cli_holder_bootstrap({**public, key: value}, HOLDER_NODE)
        for key in ("schema", "transport", "role", "node_id", "ticket"):
            missing = dict(public)
            del missing[key]
            with self.subTest(missing=key), self.assertRaises(ValueError):
                observer.cli_holder_bootstrap(missing, HOLDER_NODE)


class EvidenceTests(unittest.TestCase):
    def setUp(self):
        self.config = {"old": {"version": "0.7.1-rc.1"},
                       "new": {"version": "0.7.1-rc.2+fixture", "binary_sha256": "new"}}
        self.before = {"binary_sha256": "old"}
        self.after = {"binary_sha256": "old", "host_exit": None}

    def evidence(self, host="", command=""):
        return observer.update_evidence(self.config, self.before, self.after, host, command)

    def test_attempt_requires_expected_version_line_or_exact_new_binary(self):
        for text in ["Installing capsule", "prefix Installing 0.7.1-rc.1 → 0.7.1-rc.2+fixture...",
                     "Installing 0.7.1-rc.1 → 0.7.1-rc.20+fixture...",
                     "Installing 0.7.1-rc.0 → 0.7.1-rc.2+fixture...",
                     "Installing 0x7x1-rcx1 → 0x7x1-rcx2fixture..."]:
            with self.subTest(text=text):
                self.assertFalse(self.evidence(command=text)["attempted"])
        exact = "  Installing 0.7.1-rc.1 → 0.7.1-rc.2+fixture...\n"
        self.assertTrue(self.evidence(command=exact)["attempted"])
        self.assertTrue(self.evidence(host=exact)["attempted"])
        for digest in [None, "unrelated", "old"]:
            self.after["binary_sha256"] = digest
            self.assertFalse(self.evidence()["attempted"])
        self.after["binary_sha256"] = "new"
        self.assertTrue(self.evidence()["attempted"])
        self.before["binary_sha256"] = "new"
        self.assertFalse(self.evidence()["attempted"])

    def test_cache_stage_requires_complete_updater_line(self):
        for line in ["  Capsule cache unchanged\n", "  Cleared 2 changed cached capsule(s): chat, app-one\n"]:
            self.assertTrue(self.evidence(host=line)["cache_stage_observed"])
        for line in ["prefix Capsule cache unchanged", "Capsule cache unchanged later",
                     "Cleared 1 changed cached capsule", "Cleared 0 changed cached capsule(s): chat"]:
            self.assertFalse(self.evidence(host=line)["cache_stage_observed"])

    def test_response_loss_requires_observed_supersession_exit(self):
        for text in ["Error: operator peer returned an empty response\n",
                     "Error: connection lost\n\nCaused by:\n    timed out\n"]:
            for exit_code in [None, 0, 1, -9]:
                self.after["host_exit"] = exit_code
                self.assertFalse(self.evidence(command=text)["operator_response_lost"])
            self.after["host_exit"] = 75
            self.assertTrue(self.evidence(command=text)["operator_response_lost"])
        self.assertFalse(self.evidence(command="Error: unrelated timeout")["operator_response_lost"])

    def test_semantic_match_keeps_raw_component_hash_failure(self):
        new = {"version": "new", "binary_sha256": "binary", "components_sha256": "signed",
               "catalogue_sha256": "catalogue"}
        after = {"installed_version": "new", "binary_version": "elastos new", "binary_sha256": "binary",
                 "components_sha256": "rewritten", "components_semantic_sha256": "same-json",
                 "catalogue": {".": "catalogue"}, "preserved": {}, "host_exit": None,
                 "healthy": True, "host_lock": "held"}
        checks = observer.compare({"role": "cli", "new": new,
                                   "before": {"installed_version": "old", "preserved": {}}, "after": after,
                                   "attempted": True, "apply_exit": 0, "new_components_semantic_sha256": "same-json"})
        self.assertEqual(checks["components_content"]["status"], "passed")
        self.assertEqual(checks["components_sha256"]["status"], "failed")


class TargetIdentityTests(unittest.TestCase):
    def run_observer(self, bootstrap_did, local_did="did:key:operator"):
        # Exercise run() ordering and admission using in-memory child/HTTP fakes.
        # Only ordinary temporary JSON/log files are created.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "result"
            config = {"root": str(root), "proof_kind": "harness-self-test", "source": {}, "approval": "test",
                      "old": {"version": "old", "binary_sha256": "old"},
                      "new": {"version": "new", "binary_sha256": "new"},
                      "ports": {"publisher": 12001, "cli": 12002, "operator": 12003}}
            for role in ("publisher", "controller", "cli", "operator"):
                (root / role).mkdir()
                observer.write(root / role / "sources.json", {
                    "default_source": "fixture", "sources": [{"name": "fixture", "publisher_dids": ["did:key:publisher"]}]})
            calls, processes = [], []

            class Process:
                def __init__(self, argv, **kwargs):
                    self.role, self.args = Path(argv[0]).parent.name, argv[1:]
                    calls.append((self.role, self.args))
                    self.pid = 10000 + len(processes)
                    self.returncode = None if self.args[0] == "gateway" else 0
                    processes.append(self)
                    stream = kwargs["stdout"]
                    if self.args[:2] == ["node", "info"]:
                        did = local_did if self.role == "operator" else "did:key:" + self.role
                        stream.write(json.dumps({"did": did}).encode())
                    elif self.args == ["--version"]:
                        stream.write(b"elastos new" if stream.name.endswith("-after.log") else b"elastos old")
                    stream.flush()

                def poll(self):
                    return self.returncode

                def wait(self, timeout=None):
                    self.returncode = 0
                    return 0

            def active(role):
                return any(p.role == role and p.args[0] == "gateway" and p.poll() is None for p in processes)

            def stop(pid, _signal):
                next(p for p in processes if p.pid == pid).returncode = 0

            def bootstrap(url, timeout):
                role = next(role for role, port in config["ports"].items() if f":{port}/" in url)
                did = bootstrap_did if role == "operator" else "did:key:" + role
                return io.BytesIO(json.dumps({"schema": "elastos.carrier.bootstrap/v1", "did": did, "ticket": "fixture"}).encode())

            def snapshot(_config, role, host, version):
                return {"binary_version": version, "binary_sha256": version.split()[-1], "host_exit": host.poll()}

            with contextlib.ExitStack() as stack:
                stack.enter_context(patch.multiple(observer,
                    inspect=lambda _: {"components": {"semantic_sha256": "fixture"}},
                    data=lambda _, role: root / role, home=lambda _, role: root / role,
                    binary=lambda _, role: root / role / "elastos", snapshot=snapshot,
                    compare=lambda _: {"mock": {"status": "passed"}},
                    health=lambda port: active(next(role for role, value in config["ports"].items() if value == port)),
                    lock_state=lambda path: "held" if active(path.parent.name) else "released"))
                stack.enter_context(patch.object(observer.subprocess, "Popen", Process))
                stack.enter_context(patch.object(observer.socket, "socket"))
                stack.enter_context(patch.object(observer.os, "killpg", stop))
                stack.enter_context(patch.object(observer.time, "sleep"))
                opener = stack.enter_context(patch.object(observer.urllib.request, "build_opener"))
                opener.return_value.open.side_effect = bootstrap
                stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
                observer.run(config, output)
            return calls, observer.read(output / "result.json")

    def test_local_identity_precedes_gateway_and_matching_target_is_admitted(self):
        calls, result = self.run_observer("did:key:operator")
        info = next(i for i, (role, args) in enumerate(calls) if role == "operator" and args[:2] == ["node", "info"])
        start = next(i for i, (role, args) in enumerate(calls) if role == "operator" and args[0] == "gateway")
        admit = next(i for i, (_, args) in enumerate(calls) if args[:3] == ["node", "peer", "add"])
        self.assertLess(info, start)
        self.assertLess(start, admit)
        self.assertTrue(any(role == "controller" and args[:2] == ["node", "update"] for role, args in calls))
        self.assertEqual(result["paths"]["operator"]["status"], "passed")

    def test_spoofed_bootstrap_stops_before_peer_admission_or_operator_apply(self):
        calls, result = self.run_observer("did:key:attacker")
        self.assertFalse(any(args[:3] == ["node", "peer", "add"] for _, args in calls))
        self.assertFalse(any(args[:2] == ["node", "update"] for _, args in calls))
        self.assertIn("identity differs", result["paths"]["operator"]["reason"])

    def test_missing_local_identity_stops_before_operator_gateway(self):
        calls, result = self.run_observer("did:key:operator", local_did="")
        self.assertFalse(any(role == "operator" and args[0] == "gateway" for role, args in calls))
        self.assertIn("identity unavailable", result["paths"]["operator"]["reason"])


class CliFixtureTests(unittest.TestCase):
    """Public byte fixtures and process fakes prove observer behavior only."""

    def setUp(self):
        self.directory = tempfile.TemporaryDirectory(dir=Path(__file__).resolve().parent)
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name).resolve()
        self.manifest = {"schema": "elastos.update-hop.fixture/v1", "mode": observer.CLI_MODE,
                         "reference": "github-actions:test/repo:123", "approval": "isolated observer self-test",
                         "proof_scope": "ci-rehearsal", "proof_kind": "harness-self-test", "source": {"commit": "a" * 40, "tree": "b" * 40},
                         "old": {"version": "0.7.1-rc.1", "source": {"commit": "c" * 40, "tree": "d" * 40}},
                         "new": {"version": "0.7.1-rc.2", "source": {"commit": "e" * 40, "tree": "f" * 40}},
                         "channel": "canary", "signer_did": "did:key:zfixture", "platform": "aarch64-darwin",
                         "files": {}, "publications": {}, "selectors": {
                             "m1-install": {"positive": "old", "refusals": list(observer.CLI_REFUSALS)},
                             "m2-discovery": {"positive": "new", "refusals": list(observer.CLI_REFUSALS)}}}
        def add(name, content, mode=0o600, cid=True):
            relative = "payload/" + name
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            raw = json.dumps(content, sort_keys=True).encode() if isinstance(content, dict) else content
            path.write_bytes(raw)
            binding = {"bytes": len(raw), "sha256": hashlib.sha256(raw).hexdigest(), "mode": mode}
            if cid:
                binding["cid"] = "b" + base64.b32encode(b"\x01\x55\x12\x20" + hashlib.sha256(raw).digest()).decode().lower().rstrip("=")
            self.manifest["files"][relative] = binding
            return relative
        self.add = add
        signer = self.manifest["signer_did"]
        self.manifest["installer"] = add("install.sh", b"frozen test bytes", cid=False)
        catalogue = add("catalogue.json", {"signer_did": "did:key:zcatalog", "signature": "00" * 64,
                                              "payload": {"schema": "elastos.model.catalog/v1"}})
        components = add("components.json", {"capsules": {"test": {"cid": "qualified"}}, "external": {},
                                                "model_catalog": {"head_cid": self.manifest["files"][catalogue]["cid"],
                                                                  "publisher_dids": ["did:key:zcatalog"]}})
        old_bin, new_bin, bad_bin = (add(name, value, 0o755) for name, value in
                                    (("old-bin", b"old"), ("new-bin", b"new"), ("bad-bin", b"bad")))
        for phase in observer.CLI_PHASES:
            bin_path = old_bin if phase in ("old", "wrong-version") else bad_bin if phase == "tampered-binary" else new_bin
            def binding(relative):
                return {key: self.manifest["files"][relative][key] for key in ("cid", "sha256")} | {"size": self.manifest["files"][relative]["bytes"]}
            binary_binding = binding(bin_path)
            if phase == "tampered-binary":
                binary_binding["sha256"] = self.manifest["files"][new_bin]["sha256"]
            payload = {"schema": "elastos.release/v1", "channel": "canary",
                       "version": observer.cli_phase_version(self.manifest, phase),
                       "platforms": {"x86_64-darwin" if phase == "wrong-platform" else "aarch64-darwin": {"binary": binary_binding, "components": binding(components)}}}
            release = add(phase + "/release.json", {"payload": payload, "signature": "00" * 64,
                                                   "signer_did": "did:key:zother" if phase == "wrong-signer-release" else signer})
            head = add(phase + "/head.json", {"payload": {"schema": "elastos.release.head/v1", "channel": "canary", "version": payload["version"],
                                                         "latest_release_cid": self.manifest["files"][release]["cid"], "release_sha256": self.manifest["files"][release]["sha256"]},
                                             "signature": "00" * 64, "signer_did": "did:key:zother" if phase == "wrong-signer-head" else signer})
            receipt = add(phase + "/receipt.json", {"last_head_cid": self.manifest["files"][head]["cid"], "last_release_cid": self.manifest["files"][release]["cid"]}, cid=False)
            self.manifest["publications"][phase] = {"head": head, "release": release, "receipt": receipt,
                                                     "binary": bin_path, "components": components, "catalogue": catalogue}
        self.manifest["holder"] = {"files": {".local/bin/elastos": old_bin, observer.CLI_DATA + "/components.json": components,
                                               observer.CLI_DATA + "/bin/ipfs-provider": add("ipfs-provider", b"provider", 0o755),
                                               observer.CLI_DATA + "/bin/kubo": add("kubo", b"kubo", 0o755),
                                               observer.CLI_DATA + "/ipfs-repo/blocks/fixture": add("public-block", b"public block")}, "content": {}}
        for publication in self.manifest["publications"].values():
            for key in ("head", "release", "binary", "components", "catalogue"):
                relative = publication[key]
                self.manifest["holder"]["content"][self.manifest["files"][relative]["cid"]] = relative
        self.manifest["consumer"] = {"files": {"config/test.json": add("consumer-config.json", {"test": True}),
                                                 "state/value": add("consumer-state", b"preserve data"),
                                                 "capsules/test/entry.wasm": add("qualified-support", b"qualified support")}}
        self.manifest["build"] = add("build.json", {"schema": "elastos.update-hop.build/v1", "source": self.manifest["source"],
            "command": ["cargo", "build", "--locked", "--release", "-p", "elastos-server", "--bin", "elastos"], "status": "passed", "cleanup": {"passed": True},
            **{name: {"source": self.manifest[name]["source"], "version": self.manifest[name]["version"], "sha256": self.manifest["files"][relative]["sha256"],
                      "version_environment": self.manifest[name]["version"]} for name, relative in (("old", old_bin), ("new", new_bin))}})
        self.manifest["preserve"] = {"config": ["config"], "data": ["state"], "support": ["capsules"]}
        self.config = {"schema": self.manifest["schema"], "mode": observer.CLI_MODE, "root": str(self.root),
                       "immutable": {"reference": self.manifest["reference"], "manifest": "manifest.json"}}
        self.freeze()
        self.env = patch.dict(observer.os.environ, {"ELASTOS_CI_FIXTURE_MANIFEST_SHA256": self.config["immutable"]["sha256"],
                                                    "ELASTOS_CI_FIXTURE_REFERENCE": self.manifest["reference"], "ELASTOS_CI_REQUIRE_REAL_RUNTIME": "0", "ELASTOS_CI_FIXTURE_SCOPE": "ci-rehearsal"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def freeze(self):
        observer.write(self.root / "manifest.json", self.manifest)
        self.config["immutable"]["sha256"] = observer.digest(self.root / "manifest.json")
        if hasattr(self, "env"):
            observer.os.environ["ELASTOS_CI_FIXTURE_MANIFEST_SHA256"] = self.config["immutable"]["sha256"]

    def copy_fixture(self):
        source = self.add("copy-readme", b"public Kubo block store readme", 0o600)
        destination = self.root / "copy-home"
        target = destination / "ipfs-repo/blocks/_README"
        target.parent.mkdir(parents=True)
        target.write_bytes(b"original Kubo readme")
        target.chmod(0o444)
        return source, destination, target

    def test_copy_atomically_replaces_owned_readonly_kubo_readme(self):
        source, destination, target = self.copy_fixture()
        original_inode = target.stat().st_ino
        observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
        self.assertEqual(target.read_bytes(), (self.root / source).read_bytes())
        self.assertEqual(target.stat().st_mode & 0o777, 0o600)
        self.assertNotEqual(target.stat().st_ino, original_inode)
        self.assertEqual(list(target.parent.glob(".elastos-fixture-copy-*")), [])

    def test_copy_source_and_copied_hash_failures_preserve_original_and_cleanup(self):
        source, destination, target = self.copy_fixture()
        original = target.stat()
        for corrupt_source in (True, False):
            with self.subTest(corrupt_source=corrupt_source):
                if corrupt_source:
                    (self.root / source).write_bytes(b"changed source")
                    operation = contextlib.nullcontext()
                else:
                    (self.root / source).write_bytes(b"public Kubo block store readme")
                    operation = patch.object(observer.shutil, "copyfileobj", side_effect=lambda _, output: output.write(b"wrong copy"))
                with operation, self.assertRaisesRegex(ValueError, "fixture changed before copy|copied fixture bytes differ"):
                    observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
                self.assertEqual(target.read_bytes(), b"original Kubo readme")
                self.assertEqual(target.stat().st_ino, original.st_ino)
                self.assertEqual(target.stat().st_mode, original.st_mode)
                self.assertEqual(list(target.parent.glob(".elastos-fixture-copy-*")), [])

    def test_copy_refuses_symlink_parent_and_nonregular_targets(self):
        source, destination, target = self.copy_fixture()
        target.unlink()
        for kind in ("symlink", "directory", "fifo", "parent-symlink"):
            with self.subTest(kind=kind):
                if kind == "symlink":
                    target.symlink_to(self.root / source)
                elif kind == "directory":
                    target.mkdir()
                elif kind == "fifo":
                    os.mkfifo(target)
                else:
                    target.parent.rmdir()
                    target.parent.symlink_to(self.root / "payload", target_is_directory=True)
                with self.assertRaisesRegex(ValueError, "symlink|escapes fixture root|owned regular file"):
                    observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
                if kind == "directory":
                    target.rmdir()
                elif kind == "parent-symlink":
                    target.parent.unlink()
                    target.parent.mkdir()
                else:
                    target.unlink()
                self.assertEqual((self.root / source).read_bytes(), b"public Kubo block store readme")
                self.assertEqual(list(destination.rglob(".elastos-fixture-copy-*")), [])

    def test_copy_refuses_foreign_owned_target_or_parent(self):
        source, destination, target = self.copy_fixture()
        lstat = Path.lstat
        for foreign in (target, target.parent):
            def ownership(path):
                info = lstat(path)
                if path == foreign:
                    return SimpleNamespace(st_uid=os.geteuid() + 1, st_mode=info.st_mode)
                return info
            with self.subTest(foreign=foreign.name), patch.object(Path, "lstat", ownership), self.assertRaisesRegex(ValueError, "owned"):
                observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
            self.assertEqual(target.read_bytes(), b"original Kubo readme")
            self.assertEqual(list(target.parent.glob(".elastos-fixture-copy-*")), [])

    def test_copy_error_cleans_temporary_and_preserves_original(self):
        source, destination, target = self.copy_fixture()
        with patch.object(observer.shutil, "copyfileobj", side_effect=OSError("copy interrupted")), self.assertRaisesRegex(OSError, "copy interrupted"):
            observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
        self.assertEqual(target.read_bytes(), b"original Kubo readme")
        self.assertEqual(target.stat().st_mode & 0o777, 0o444)
        self.assertEqual(list(target.parent.glob(".elastos-fixture-copy-*")), [])

    def test_copy_preserves_target_replaced_during_preparation(self):
        source, destination, target = self.copy_fixture()
        copy = shutil.copyfileobj
        def replace_target(input_stream, output):
            copy(input_stream, output)
            changed = target.with_name("changed")
            changed.write_bytes(b"new owner-written target")
            os.replace(changed, target)
        with patch.object(observer.shutil, "copyfileobj", side_effect=replace_target), self.assertRaisesRegex(ValueError, "target changed before replacement"):
            observer.cli_copy(self.root, self.manifest, {"ipfs-repo/blocks/_README": source}, destination)
        self.assertEqual(target.read_bytes(), b"new owner-written target")
        self.assertEqual(list(target.parent.glob(".elastos-fixture-copy-*")), [])

    def coordination_fixture(self):
        home = self.root / "coordination-home"
        directory = home / observer.CLI_DATA
        directory.mkdir(parents=True)
        binary = home / ".local/bin/elastos"
        binary.parent.mkdir(parents=True)
        binary.write_bytes(b"old")
        for name in ("components.json", "model-catalog.json"):
            observer.write(directory / name, {})
        observer.write(directory / "sources.json", {"default_source": "default", "sources": [{}]})
        observer.cli_copy(self.root, self.manifest, self.manifest["consumer"]["files"], directory)
        lock = directory / "host-process.lock"
        observer.write(lock, {"pid": 12345, "role": "principal-root-upgrade", "addr": "offline"})
        lock.chmod(0o600)
        return home, lock

    def installed_metadata_fixture(self, home, name="old", consumed=True):
        directory = home / observer.CLI_DATA / ("installation" if consumed else observer.CLI_PUBLISHER)
        directory.mkdir(mode=0o700, parents=True, exist_ok=True)
        directory.chmod(0o700)
        for key, filename in (("head", "release-head.json"), ("release", "release.json")):
            path = directory / filename
            path.write_bytes((self.root / self.manifest["publications"][name][key]).read_bytes())
            path.chmod(0o600)
        return directory

    def test_consumed_metadata_is_authoritative_snapshot_and_publisher_is_preserved_data(self):
        home, _ = self.coordination_fixture()
        publisher = self.installed_metadata_fixture(home, consumed=False)
        legacy = observer.cli_state(self.manifest, home)
        self.assertIsNone(legacy["installed_metadata"])
        self.assertEqual(observer.cli_legacy_installed_metadata(home), observer.cli_expected_installed_metadata(self.manifest, "old"))
        for path in publisher.iterdir():
            path.chmod(0o644)  # Frozen installer metadata uses its historical accepted mode.
        self.assertEqual(observer.cli_legacy_installed_metadata(home), observer.cli_expected_installed_metadata(self.manifest, "old"))
        legacy_release = publisher / "release.json"
        legacy_release.chmod(0o666)
        with self.assertRaisesRegex(ValueError, "legacy installed metadata ownership/mode/bound"):
            observer.cli_legacy_installed_metadata(home)
        legacy_release.chmod(0o644)
        installed = self.installed_metadata_fixture(home, "new")
        current = observer.cli_state(self.manifest, home)
        self.assertEqual(current["installed_metadata"], observer.cli_expected_installed_metadata(self.manifest, "new"))
        self.assertEqual(legacy["data"], current["data"])
        self.assertTrue(all(relative not in current["data"] for relative in observer.CLI_INSTALLED_METADATA))
        self.assertIn(observer.CLI_PUBLISHER + "/release.json", current["data"])
        (installed / "release.json").write_bytes(b"substituted consumed release")
        changed = observer.cli_state(self.manifest, home)
        self.assertNotEqual(current, changed)
        self.assertEqual(current["data"], changed["data"])
        (publisher / "release.json").write_bytes(b"changed publication")
        self.assertNotEqual(changed["data"], observer.cli_state(self.manifest, home)["data"])

    def test_consumed_metadata_refuses_partial_unsafe_or_unknown_inventory(self):
        for case in ("partial", "symlink file", "symlink directory", "file mode", "directory mode", "hard link", "unknown", "fifo"):
            with self.subTest(case=case):
                home = self.root / ("metadata-" + case.replace(" ", "-"))
                installed = self.installed_metadata_fixture(home)
                target = installed / "release.json"
                if case == "partial":
                    target.unlink()
                elif case == "symlink file":
                    target.unlink()
                    target.symlink_to(self.root / self.manifest["publications"]["old"]["release"])
                elif case == "symlink directory":
                    other = installed.with_name("foreign-installation")
                    installed.rename(other)
                    installed.symlink_to(other, target_is_directory=True)
                elif case == "file mode":
                    target.chmod(0o644)
                elif case == "directory mode":
                    installed.chmod(0o755)
                elif case == "hard link":
                    os.link(target, home / "foreign-link")
                elif case == "unknown":
                    (installed / "migration-scratch").write_bytes(b"unclassified authority")
                else:
                    target.unlink()
                    os.mkfifo(target, 0o600)
                with self.assertRaises((ValueError, OSError)):
                    observer.cli_installed_metadata(home)

    def test_migration_scratch_remains_in_ordinary_preservation_snapshot(self):
        home, _ = self.coordination_fixture()
        before = observer.cli_state(self.manifest, home)
        stage = home / observer.CLI_DATA / ".elastos.installation-migrate"
        stage.mkdir(mode=0o700)
        (stage / "release.json").write_bytes(b"incomplete migration")
        after = observer.cli_state(self.manifest, home)
        self.assertNotEqual(before["data"], after["data"])
        self.assertIn(".elastos.installation-migrate/release.json", after["data"])

    def test_consumed_metadata_refuses_foreign_file_owner_and_oversize(self):
        home = self.root / "metadata-owner"
        installed = self.installed_metadata_fixture(home)
        fstat = observer.os.fstat
        def foreign_file(descriptor):
            info = fstat(descriptor)
            return SimpleNamespace(st_mode=info.st_mode, st_uid=os.geteuid() + 1,
                                   st_nlink=info.st_nlink, st_size=info.st_size)
        with patch.object(observer.os, "fstat", side_effect=foreign_file), self.assertRaisesRegex(ValueError, "file ownership"):
            observer.cli_installed_metadata(home)
        (installed / "release.json").write_bytes(b"x" * (256 * 1024 + 1))
        with self.assertRaisesRegex(ValueError, "file ownership/mode/bound"):
            observer.cli_installed_metadata(home)

    def test_coordination_pid_change_is_separate_from_user_data_and_other_locks(self):
        home, lock = self.coordination_fixture()
        other = lock.with_name("user.lock")
        other.write_bytes(b"user data")
        before = observer.cli_state(self.manifest, home)
        observer.write(lock, {"pid": 54321, "role": "principal-root-upgrade", "addr": "offline"})
        after = observer.cli_state(self.manifest, home)
        self.assertNotEqual(before["coordination"], after["coordination"])
        self.assertEqual(before["data"], after["data"])
        self.assertEqual(after["coordination"]["status"], "released")
        self.assertNotIn("host-process.lock", after["data"])
        self.assertIn("user.lock", after["data"])
        other.write_bytes(b"changed user data")
        self.assertNotEqual(after["data"], observer.cli_state(self.manifest, home)["data"])

    def test_coordination_refuses_held_flock(self):
        home, lock = self.coordination_fixture()
        with lock.open("rb") as owner:
            observer.fcntl.flock(owner, observer.fcntl.LOCK_EX | observer.fcntl.LOCK_NB)
            with self.assertRaisesRegex(ValueError, "lock is held"):
                observer.cli_state(self.manifest, home)
        self.assertEqual(observer.cli_coordination(home)["status"], "released")

    def test_coordination_refuses_malformed_metadata_and_unsafe_mode(self):
        home, lock = self.coordination_fixture()
        valid = {"pid": 12345, "role": "principal-root-upgrade", "addr": "offline"}
        malformed = [{**valid, "pid": value} for value in (True, 0, -1, "12345", 0x100000000)]
        malformed += [{**valid, "role": "gateway"}, {**valid, "addr": "127.0.0.1:1"}, {**valid, "extra": True}, {"pid": 12345}]
        for metadata in malformed:
            with self.subTest(metadata=metadata), self.assertRaisesRegex(ValueError, "metadata differs"):
                observer.write(lock, metadata)
                observer.cli_state(self.manifest, home)
        lock.write_text('{"pid":1,"pid":2,"role":"principal-root-upgrade","addr":"offline"}')
        with self.assertRaisesRegex(ValueError, "repeats a field"):
            observer.cli_state(self.manifest, home)
        observer.write(lock, valid)
        lock.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "file mode differs"):
            observer.cli_state(self.manifest, home)

    def test_coordination_refuses_symlink_and_foreign_owned_file(self):
        home, lock = self.coordination_fixture()
        original = lock.with_name("original")
        lock.rename(original)
        lock.symlink_to(original)
        with self.assertRaisesRegex(ValueError, "symlink"):
            observer.cli_state(self.manifest, home)
        lock.unlink()
        original.rename(lock)
        lstat = Path.lstat
        def foreign_owner(path):
            info = lstat(path)
            return SimpleNamespace(st_uid=os.geteuid() + 1, st_mode=info.st_mode) if path == lock else info
        with patch.object(Path, "lstat", foreign_owner), self.assertRaisesRegex(ValueError, "owned regular file"):
            observer.cli_state(self.manifest, home)

    def admit(self, local_rehearsal=None):
        with patch.object(observer, "cli_installer_metadata", return_value=[self.manifest["signer_did"], ""]), patch.object(observer, "cli_signature") as signatures:
            admitted = observer.cli_admit(self.config, local_rehearsal)
            self.assertEqual(signatures.call_count, 3 * len(self.manifest["publications"]))
            return admitted

    def local_fixture(self):
        reference = "local-rehearsal:" + self.manifest["source"]["commit"] + ":UP-03-test"
        self.manifest.update(proof_scope="local-rehearsal", reference=reference)
        self.config["immutable"]["reference"] = reference
        build = observer.cli_json(self.root / self.manifest["build"])
        build.update(proof_scope="local-rehearsal", reference=reference)
        for name in ("old", "new"):
            self.manifest[name]["source"] = dict(self.manifest["source"])
            build[name]["source"] = dict(self.manifest["source"])
        self.manifest["build"] = self.add("local-build.json", build, cid=False)
        self.freeze()
        return reference

    @contextlib.contextmanager
    def local_environment(self):
        source = dict(self.manifest["source"])
        def git(argv, **kwargs):
            if argv[1] == "status":
                return ""
            return source["commit" if argv[-1] == "HEAD" else "tree"] + "\n"
        with patch.dict(observer.os.environ, {"CI": "", "GITHUB_ACTIONS": "", "ELASTOS_CI_FIXTURE_REFERENCE": "", "ELASTOS_CI_FIXTURE_SCOPE": ""}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
             patch.object(observer.subprocess, "check_output", side_effect=git):
            yield

    def test_local_scope_requires_its_explicit_selector_and_preserves_all_refusals(self):
        reference = self.local_fixture()
        with self.local_environment():
            with self.assertRaisesRegex(ValueError, "proof scope differs"):
                self.admit()
            self.assertEqual(self.admit(reference)["proof_scope"], "local-rehearsal")
            with patch.object(observer, "cli_installer_metadata", return_value=[self.manifest["signer_did"], ""]), patch.object(observer, "cli_signature"):
                receipt = observer.cli_inspect(self.config, reference)
            self.assertEqual(receipt["reference"], reference)
            self.assertEqual(receipt["retrieval_reference"], "")
            for selector in receipt["selectors"].values():
                self.assertEqual(selector["refusals"], list(observer.CLI_REFUSALS))
            with self.assertRaisesRegex(ValueError, "scope or reference differs"):
                self.admit(reference.replace("UP-03-test", "other-test"))

    def test_local_selector_refuses_hosted_authority_dirty_source_and_candidate_substitution(self):
        reference = self.local_fixture()
        for variable, value, expected in (
            ("CI", "true", "hosted CI authority"),
            ("GITHUB_ACTIONS", "true", "hosted CI authority"),
            ("ELASTOS_CI_FIXTURE_REFERENCE", "github-actions:test/repo:123", "artifact retrieval"),
            ("ELASTOS_CI_FIXTURE_SCOPE", "production-positive", "proof scope differs"),
            ("ELASTOS_CI_FIXTURE_SCOPE", "ci-rehearsal", "proof scope differs"),
        ):
            with self.subTest(variable=variable, value=value), self.local_environment(), patch.dict(observer.os.environ, {variable: value}), \
                 self.assertRaisesRegex(ValueError, expected):
                observer.cli_local_rehearsal(reference)
        with self.local_environment():
            with patch.object(observer.subprocess, "check_output", return_value=" M scripts/update-hop-compare.py\n"), \
                 self.assertRaisesRegex(ValueError, "clean admitted source"):
                observer.cli_local_rehearsal(reference)
            with self.assertRaisesRegex(ValueError, "source commit differs"):
                observer.cli_local_rehearsal(reference.replace("a" * 40, "c" * 40))
            with patch.object(observer.sys, "platform", "linux"), self.assertRaisesRegex(ValueError, "native Mac"):
                observer.cli_local_rehearsal(reference)
            with self.assertRaisesRegex(ValueError, "exact source commit"):
                observer.cli_local_rehearsal("local-rehearsal:short:fixture")

    def test_local_fixture_refuses_scope_source_and_build_receipt_substitution(self):
        reference = self.local_fixture()
        with self.local_environment():
            for scope in ("production-positive", "ci-rehearsal"):
                self.manifest["proof_scope"] = scope
                self.freeze()
                with self.assertRaisesRegex(ValueError, "scope or reference differs"):
                    self.admit(reference)
            self.manifest["proof_scope"] = "local-rehearsal"
            self.manifest["source"]["tree"] = "c" * 40
            self.freeze()
            with self.assertRaisesRegex(ValueError, "source tree differs"):
                self.admit(reference)
            self.manifest["source"]["tree"] = "b" * 40
            self.manifest["old"]["source"]["commit"] = "c" * 40
            self.freeze()
            with self.assertRaisesRegex(ValueError, "compiled Runtime source differs"):
                self.admit(reference)
            self.manifest["old"]["source"] = dict(self.manifest["source"])
            original = observer.cli_json(self.root / self.manifest["build"])
            for field, value, expected in (
                ("proof_scope", "ci-rehearsal", "hop proof scope"),
                ("reference", reference + "-other", "local hop reference"),
            ):
                build = {**original, field: value}
                self.manifest["build"] = self.add("substituted-build-" + field + ".json", build, cid=False)
                self.freeze()
                with self.assertRaisesRegex(ValueError, expected):
                    self.admit(reference)

    def test_local_manifest_reference_cannot_be_relabelled_as_ci_proof(self):
        self.local_fixture()
        self.manifest["proof_scope"] = "ci-rehearsal"
        self.freeze()
        with patch.dict(observer.os.environ, {"ELASTOS_CI_FIXTURE_SCOPE": "ci-rehearsal"}), \
             self.assertRaisesRegex(ValueError, "local fixture requires its explicit selector"):
            self.admit()
        self.positive()
        self.manifest["reference"] = self.config["immutable"]["reference"] = "fixture:relabelled"
        self.freeze()
        with self.assertRaisesRegex(ValueError, "local build requires its explicit selector"):
            self.admit()

    def test_local_option_refuses_capacity_compare_and_non_cli_fixtures(self):
        reference = "local-rehearsal:" + "a" * 40 + ":UP-03-test"
        for operation, fixture in (("prepare-ci-disk", {}), ("compare", {}), ("inspect", {"mode": "comparison"}), ("run", {"mode": "comparison"})):
            argv = ["observer", operation, str(self.root / "fixture.json"), "--local-rehearsal", reference]
            with self.subTest(operation=operation), patch.object(observer.sys, "argv", argv), \
                 patch.object(observer, "read", return_value=fixture), patch.object(observer.os, "umask"), patch.object(observer.signal, "signal"), \
                 contextlib.redirect_stderr(io.StringIO()) as stderr:
                self.assertEqual(observer.main(), 2)
            self.assertIn("local rehearsal applies only", stderr.getvalue())

    def positive(self):
        self.manifest["proof_scope"] = "production-positive"
        self.manifest["publications"] = {name: self.manifest["publications"][name] for name in ("old", "new")}
        for selector in self.manifest["selectors"].values():
            selector["refusals"] = []
        observer.os.environ["ELASTOS_CI_FIXTURE_SCOPE"] = "production-positive"
        self.freeze()

    def test_real_positive_scope_admits_without_negative_signing_inputs(self):
        self.positive()
        self.assertEqual(self.admit()["proof_scope"], "production-positive")
        code, calls, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertEqual(result["proof_scope"], "production-positive")
        self.assertEqual([argv[1:] for argv, _ in calls if argv[1] == "update"],
                         [["update", "--check"], ["update"], ["update"]])
        self.assertEqual(set(result["paths"]["m2-discovery"]["checks"]), {"check", "apply", "repeat"})

    def test_disposable_package_cannot_enter_real_positive_admission(self):
        observer.os.environ["ELASTOS_CI_FIXTURE_SCOPE"] = "production-positive"
        with self.assertRaisesRegex(ValueError, "proof scope differs"):
            self.admit()

    def test_disposable_hop_requires_two_binary_hashes_and_compiled_version_receipt(self):
        pub = self.manifest["publications"]["new"]
        original = pub["binary"]
        pub["binary"] = self.manifest["publications"]["old"]["binary"]
        self.freeze()
        with self.assertRaisesRegex(ValueError, "artifact CID/size|artifact hash|different old/new"):
            self.admit()
        pub["binary"] = original
        build_path = self.root / self.manifest["build"]
        build = observer.cli_json(build_path)
        build["new"]["version_environment"] = self.manifest["old"]["version"]
        relative = self.add("wrong-build.json", build)
        self.manifest["build"] = relative
        self.freeze()
        with self.assertRaisesRegex(ValueError, "compiled version input"):
            self.admit()

    def test_admission_requires_complete_public_package_and_independent_pin(self):
        self.assertEqual(self.admit()["proof_kind"], "harness-self-test")
        observer.os.environ["ELASTOS_CI_FIXTURE_MANIFEST_SHA256"] = "0" * 64
        with self.assertRaisesRegex(ValueError, "independently pinned"):
            observer.cli_admit(self.config)

    def test_payload_parity_extra_files_and_symlinks_are_refused(self):
        path = self.root / self.manifest["installer"]
        original = path.read_bytes()
        path.write_bytes(b"altered")
        with self.assertRaisesRegex(ValueError, "size differs|hash differs"):
            self.admit()
        path.write_bytes(original)
        extra = self.root / "payload/extra"
        extra.write_text("unlisted")
        with self.assertRaisesRegex(ValueError, "incomplete"):
            self.admit()
        extra.unlink()
        path.unlink()
        path.symlink_to(self.root / "payload/kubo")
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.admit()

    def test_outer_retrieval_cannot_repin_inner_snapshot_or_mock_proof(self):
        observer.os.environ["ELASTOS_CI_FIXTURE_REFERENCE"] = "github-actions:test/repo:456"
        self.admit()
        self.config["immutable"]["reference"] = "fixture:substituted"
        with self.assertRaisesRegex(ValueError, "manifest identity"):
            self.admit()
        self.config["immutable"]["reference"] = self.manifest["reference"]
        observer.os.environ["ELASTOS_CI_REQUIRE_REAL_RUNTIME"] = "1"
        with self.assertRaisesRegex(ValueError, "requires real-runtime"):
            self.admit()

    def test_actual_signature_helper_and_native_binary_admission_refuse_stubs(self):
        with self.assertRaisesRegex(ValueError, "signature invalid"):
            observer.cli_signature(Path(__file__).with_name("install.sh"),
                                   self.root / self.manifest["publications"]["old"]["head"],
                                   "elastos.release.head.v1", self.manifest["signer_did"], observer.cli_environment(self.root))
        with self.assertRaisesRegex(ValueError, "Mach-O"):
            observer.cli_macho(self.root / self.manifest["publications"]["old"]["binary"], "aarch64-darwin")
        native = self.root / "header-only"
        native.write_bytes(b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16)
        observer.cli_macho(native, "aarch64-darwin")
        with self.assertRaisesRegex(ValueError, "architecture"):
            observer.cli_macho(native, "x86_64-darwin")

    def test_stamped_real_installer_defaults_are_read_without_execution(self):
        actual = Path(__file__).with_name("install.sh").read_text()
        installer = self.root / "stamped-install.sh"
        stamped = actual.replace("__MAINTAINER_DID__", self.manifest["signer_did"]).replace("__HEAD_CID__", "")
        installer.write_text(stamped)
        self.assertEqual(observer.cli_installer_metadata(installer, observer.cli_environment(self.root)),
                         [self.manifest["signer_did"], ""])
        installer.write_text(stamped.replace('HEAD_CID="${ELASTOS_HEAD_CID:-}"',
                                            'HEAD_CID="${ELASTOS_HEAD_CID:-pinned-head}"'))
        self.assertEqual(observer.cli_installer_metadata(installer, {})[1], "pinned-head")
        with self.assertRaisesRegex(ValueError, "trust override"):
            observer.cli_installer_metadata(installer, {"ELASTOS_HEAD_CID": ""})
        installer.write_text(stamped + '\nMAINTAINER_DID="${ELASTOS_MAINTAINER_DID:-substituted}"\n')
        with self.assertRaisesRegex(ValueError, "literal trust default"):
            observer.cli_installer_metadata(installer, {})

    def test_cleanup_does_not_signal_recycled_historic_process_groups(self):
        class Exited:
            pid, returncode = 30000, 0
            def poll(self):
                return 0
            def wait(self, timeout=None):
                return 0
        manager = observer.CliProcesses(self.root)
        manager.processes = [Exited()]
        unrelated = {"pid": 30001, "parent": 1, "group": 30000, "command": "/unowned/process"}
        with patch.object(observer, "cli_census", return_value=[unrelated]), patch.object(observer.os, "killpg") as groups, patch.object(observer.os, "kill") as pids, patch.object(observer.time, "sleep"):
            cleanup = manager.cleanup()
        groups.assert_not_called()
        pids.assert_not_called()
        self.assertTrue(cleanup["errors"])

    def test_cleanup_stops_verified_orphan_root_and_its_group_children(self):
        manager = observer.CliProcesses(self.root)
        executable = self.root / "payload/kubo"
        manager.roots[str(executable)] = {observer.digest(executable)}
        rows = [{"pid": 31000, "parent": 1, "group": 31000, "command": str(executable)},
                {"pid": 31001, "parent": 31000, "group": 31000, "command": "/usr/bin/owned-child"}]
        def kill(pid, sig):
            rows[:] = [row for row in rows if row["pid"] != pid]
        with patch.object(observer, "cli_census", side_effect=lambda: list(rows)), patch.object(observer.os, "kill", side_effect=kill) as pids, patch.object(observer.time, "sleep"):
            cleanup = manager.cleanup()
        self.assertEqual({call.args[0] for call in pids.call_args_list}, {31000, 31001})
        self.assertEqual(cleanup["remaining_pids"], [])
        self.assertFalse(cleanup["errors"])

    def test_paths_private_json_and_phase_mapping_are_refused(self):
        with self.assertRaisesRegex(ValueError, "noncanonical"):
            observer.cli_path(self.root, "payload/../outside")
        relative = self.add("kubo-config.json", {"Identity": {"PrivKey": "private operator material"}})
        self.freeze()
        with self.assertRaisesRegex(ValueError, "private key"):
            self.admit()
        (self.root / relative).unlink()
        del self.manifest["files"][relative]
        self.manifest["selectors"]["m2-discovery"]["positive"] = "old"
        self.freeze()
        with self.assertRaisesRegex(ValueError, "phase mapping"):
            self.admit()

    def test_wrong_signer_and_receipt_admission_cannot_skip_verification(self):
        with patch.object(observer, "cli_installer_metadata", return_value=[self.manifest["signer_did"], ""]), patch.object(observer, "cli_signature", side_effect=ValueError("fixture envelope signature invalid")):
            with self.assertRaisesRegex(ValueError, "signature invalid"):
                observer.cli_admit(self.config)
        self.manifest["publications"]["wrong-signer-head"] = self.manifest["publications"]["wrong-version"]
        self.freeze()
        with self.assertRaisesRegex(ValueError, "signer phase"):
            self.admit()

    def test_refusal_requires_exact_boundary_unchanged_files_and_real_failure(self):
        failure = {"exit": 1}
        self.assertEqual(observer.cli_refusal("wrong-signer-head", failure, "Checking for updates", "Carrier connection failed", True)["status"], "failed")
        self.assertEqual(observer.cli_refusal("wrong-signer-release", failure, "Fetching release:", "Signer DID mismatch", True)["status"], "passed")
        self.assertEqual(observer.cli_refusal("wrong-signer-head", failure, "Fetching release:", "Signer DID mismatch", True)["status"], "failed")
        for code, unchanged in ((0, True), (124, True), (1, False)):
            self.assertEqual(observer.cli_refusal("tampered-binary", {"exit": code}, "Downloading binary", "SHA-256 mismatch", unchanged)["status"], "failed")
        self.assertEqual(observer.cli_refusal("tampered-binary", failure, "Downloading binary\nDownloading components", "SHA-256 mismatch", True)["status"], "failed")
        for case, stdout, stderr in (("wrong-platform", "Fetching release:", "No binary CID for platform aarch64-darwin"),
                                     ("wrong-version", "Downloading binary\nDownloading components", "Installed binary version mismatch")):
            self.assertEqual(observer.cli_refusal(case, failure, stdout, stderr, True)["status"], "passed")
            for changed_stdout, changed_stderr, unchanged in ((stdout, "Carrier connection failed", True), (stdout, stderr, False)):
                self.assertEqual(observer.cli_refusal(case, failure, changed_stdout, changed_stderr, unchanged)["status"], "failed")

    def qualified_home_support(self, support=None, marker=b""):
        support = support or self.root / "support"
        (support / "bin").mkdir(parents=True, exist_ok=True)
        for name in (*observer.CLI_NATIVE_SETUP, "custody-provider"):
            path = support / "bin" / name
            path.write_bytes(marker + name.encode())
            path.chmod(0o755)
        registry = {"capsules": {}, "external": {name: {
            "install_path": "bin/" + name, "platforms": {"darwin-arm64": {"checksum": "sha256:" + observer.digest(support / "bin" / name)}}}
            for name in observer.CLI_NATIVE_SETUP}}
        for name in observer.CLI_WEB_CAPSULES:
            capsule = support / "capsules" / name
            (capsule / "browser").mkdir(parents=True, exist_ok=True)
            observer.write(capsule / "capsule.json", {"schema": "elastos.capsule/v1", "name": name, "role": "app",
                           "type": "wasm", "entrypoint": "browser/index.html", "execution": "web-projection"})
            (capsule / "browser/index.html").write_bytes(b"<html>installed " + marker + name.encode() + b" fixture</html>")
            (capsule / "browser/shell.js").write_bytes(b"export const home = true;")
            document = capsule / "browser/index.html"
            registry["capsules"][name] = {"cid": "", "sha256": "", "size": 0, "install_path": "capsules/" + name,
                "entrypoint": "browser/index.html", "entrypoint_sha256": "sha256:" + observer.digest(document),
                "entrypoint_size": document.stat().st_size, "platforms": ["darwin-arm64"],
                "browser_assets": [{"path": path.relative_to(capsule).as_posix(), "sha256": "sha256:" + observer.digest(path),
                                    "size": path.stat().st_size} for path in sorted((capsule / "browser").rglob("*")) if path.is_file()]}
        for name, provides in (("ipfs-provider", "elastos://ipfs/*"), ("localhost-provider", "localhost://*")):
            registry["external"][name]["provider_runtime"] = {"role": "provider", "substrate": "native",
                "runtime_abi": "elastos.provider-stdio/v1", "execution": "native-provider", "provides": provides}
        registry["external"]["home"] = {"install_path": "capsules/home", "description": "Qualified source Home",
            "platforms": {"*": {"release_path": "home.tar.gz", "extract_path": "home", "install_path": "capsules/home"}}}
        registry["external"]["custody-provider"] = {"install_path": "bin/custody-provider", "platforms": {
            "darwin-arm64": {"release_path": "custody-provider-darwin-arm64", "install_path": "bin/custody-provider"}}}
        observer.write(support / "components.json", registry)
        return support

    def previous_release(self, runtime, version="0.7.1"):
        """A maintainer-signed published release laid out as fetch-previous-release writes it."""
        previous = self.root / "previous"
        support = self.qualified_home_support(previous / "support", b"published ")
        (previous / "elastos").write_bytes(runtime.read_bytes())
        (previous / "elastos").chmod(0o755)
        observer.write(previous / "components.json", {"schema": "elastos.components/v1", "external": {
            name: {"platforms": {"darwin-arm64": {"cid": "Qm" + name, "release_path": name + "-darwin-arm64", "size": 1,
                                                  "checksum": "sha256:" + observer.digest(support / "bin" / name)}}}
            for name in observer.CLI_NATIVE_SETUP}})
        (previous / "install.sh").write_text(Path(observer.__file__).with_name("install.sh").read_text())
        source = {"commit": "9" * 40, "tree": "8" * 40}
        artifacts = {key: {"cid": "Qm" + key, "sha256": observer.digest(previous / name), "size": (previous / name).stat().st_size}
                     for key, name in (("binary", "elastos"), ("components", "components.json"))}
        envelope = signing_fixture.sign_envelope({"schema": "elastos.release/v1", "channel": "canary", "version": version,
                                                  "source": source, "platforms": {"aarch64-darwin": artifacts}},
                                                 "elastos.release.v1", b"\x07" * 32)
        observer.write(previous / "release.json", envelope)
        raw = (previous / "release.json").read_bytes()
        observer.write(self.root / "previous-pin.json", {
            "schema": "elastos.update-hop.previous-release/v1", "version": version, "signer_did": envelope["signer_did"],
            "release_cid": "b" + base64.b32encode(b"\x01\x55\x12\x20" + hashlib.sha256(raw).digest()).decode().lower().rstrip("="),
            "release_sha256": hashlib.sha256(raw).hexdigest()})
        pin = patch.object(observer, "CLI_PREVIOUS_PIN", self.root / "previous-pin.json")
        pin.start()
        self.addCleanup(pin.stop)
        return previous, source

    def qualified_localhost_metadata(self, support, fault=None):
        files = {"capsule.json": json.dumps({"schema": "elastos.capsule/v1", "name": "localhost-provider",
                 "version": "0.1.0", "role": "provider", "type": "microvm", "entrypoint": "rootfs.ext4",
                 "icon": "icons", "provides": "localhost://*"}).encode(),
                 **{"icons/icon-" + str(size) + ".png": b"fixture icon " + str(size).encode() for size in (32, 64, 128, 256)}}
        if fault == "missing manifest":
            del files["capsule.json"]
        elif fault == "missing icon":
            del files["icons/icon-256.png"]
        output = io.BytesIO()
        with observer.tarfile.open(fileobj=output, mode="w:gz") as archive:
            for name in ("localhost-provider", "localhost-provider/icons"):
                member = observer.tarfile.TarInfo(name)
                member.type, member.mode = observer.tarfile.DIRTYPE, 0o755
                archive.addfile(member)
            for name, raw in sorted(files.items()):
                member = observer.tarfile.TarInfo("localhost-provider/" + name)
                member.size, member.mode = len(raw), 0o644
                archive.addfile(member, io.BytesIO(raw))
            if fault in ("traversal", "link", "hard link", "unsafe mode"):
                member = observer.tarfile.TarInfo("../escaped" if fault == "traversal" else "localhost-provider/extra")
                member.mode = 0o777 if fault == "unsafe mode" else 0o644
                if fault in ("link", "hard link"):
                    member.type = observer.tarfile.SYMTYPE if fault == "link" else observer.tarfile.LNKTYPE
                    member.linkname = "../../foreign"
                archive.addfile(member, io.BytesIO(b""))
            elif fault == "entry count":
                for index in range(1024):
                    member = observer.tarfile.TarInfo("localhost-provider/empty-" + str(index))
                    member.type, member.mode = observer.tarfile.DIRTYPE, 0o755
                    archive.addfile(member)
            elif fault == "oversized member":
                member = observer.tarfile.TarInfo("localhost-provider/oversized")
                member.size, member.mode = 16 * 1024**2 + 1, 0o644
                archive.addfile(member)
        path = self.root / observer.CLI_LOCALHOST_METADATA
        path.write_bytes(b"invalid archive bytes" if fault == "invalid tar" else output.getvalue())
        path.chmod(0o600)
        registry = observer.cli_json(support / "components.json")
        registry["external"]["localhost-provider"]["capsule_metadata"] = {
            "install_path": "capsules/localhost-provider", "platforms": {"*": {
                "checksum": "sha256:" + observer.digest(path), "size": path.stat().st_size,
                "release_path": observer.CLI_LOCALHOST_METADATA, "extract_path": "localhost-provider",
                "install_path": "capsules/localhost-provider"}}}
        observer.write(support / "components.json", registry)
        return path

    def setup_fixture(self, initial_home=False, support=None, metadata=False):
        support = support or self.qualified_home_support()
        metadata_path = self.qualified_localhost_metadata(support) if metadata else None
        entries, paths = {}, []
        for name in observer.CLI_WEB_CAPSULES:
            entries[name], capsule_paths, _ = observer.cli_qualified_home(support, "darwin-arm64", name)
            paths.extend(capsule_paths)
        mapping = {}
        for path in [*(support / "bin" / name for name in observer.CLI_NATIVE_SETUP), *paths]:
            target = path.relative_to(support).as_posix()
            mapping[target] = self.add("setup/" + target, path.read_bytes(), 0o755 if target.startswith("bin/") else 0o600)
        mapping["fixture-tools/open"] = self.add("setup/opener", b"fixture no-op utility", 0o700)
        self.manifest["consumer"]["files"].update(mapping)
        for relative in mapping.values():
            self.manifest["holder"]["content"][self.manifest["files"][relative]["cid"]] = relative
        artifacts = {}
        for name in observer.CLI_WEB_CAPSULES:
            archive = self.add("setup/" + name + ".tar.gz", observer.cli_home_archive(self.root, self.manifest, mapping, name))
            self.manifest["holder"]["content"][self.manifest["files"][archive]["cid"]] = archive
            artifacts[name + ".tar.gz"] = archive
        artifacts.update({name: mapping["bin/" + name] for name in observer.CLI_NATIVE_SETUP})
        self.manifest["setup"] = {"artifacts": artifacts}
        components = observer.cli_json(self.root / self.manifest["publications"]["old"]["components"])
        components["schema"] = "elastos.components/v1"
        components["profiles"] = {"home": {"components": list(observer.CLI_SETUP_COMPONENTS)}}
        for name, artifact in zip(observer.CLI_SETUP_COMPONENTS, observer.CLI_SETUP_ARTIFACTS):
            binding = self.manifest["files"][artifacts[artifact]]
            target = "capsules/" + name if name in observer.CLI_WEB_CAPSULES else "bin/" + name
            selected = {"release_path": artifact, "install_path": target, "cid": binding["cid"],
                        "checksum": "sha256:" + binding["sha256"], "size": binding["bytes"]}
            if name in observer.CLI_WEB_CAPSULES:
                selected["extract_path"] = name
                components["capsules"][name] = {**entries[name], "cid": binding["cid"],
                    "sha256": "sha256:" + binding["sha256"], "size": binding["bytes"]}
            components["external"][name] = {"install_path": target, "platforms": {"darwin-arm64": selected}}
        if metadata_path is not None:
            qualified = observer.cli_json(support / "components.json")
            descriptor = qualified["external"]["localhost-provider"]
            selected = observer.cli_localhost_metadata_info(descriptor, "darwin-arm64")
            expected = observer.cli_localhost_metadata_files(metadata_path.read_bytes(), selected)
            relative = self.add("setup/" + observer.CLI_LOCALHOST_METADATA, metadata_path.read_bytes())
            self.manifest["holder"]["content"][self.manifest["files"][relative]["cid"]] = relative
            selected["cid"] = self.manifest["files"][relative]["cid"]
            components["external"]["localhost-provider"]["capsule_metadata"] = descriptor["capsule_metadata"]
            self.manifest["setup"]["artifacts"][observer.CLI_LOCALHOST_METADATA] = relative
            self.manifest["setup"]["localhost_metadata"] = {"artifact": observer.CLI_LOCALHOST_METADATA, "files": sorted(expected)}
            for target, (raw, mode) in expected.items():
                relative = self.add("setup/" + target, raw, mode)
                mapping[target] = relative
                self.manifest["consumer"]["files"][target] = relative
                self.manifest["holder"]["content"][self.manifest["files"][relative]["cid"]] = relative
        relative = self.add("components.json", components)
        binding = self.manifest["files"][relative]
        self.manifest["holder"]["content"][binding["cid"]] = relative
        for phase, publication in self.manifest["publications"].items():
            release = observer.cli_json(self.root / publication["release"])
            selected = next(iter(release["payload"]["platforms"].values()))
            selected["components"] = {"cid": binding["cid"], "sha256": binding["sha256"], "size": binding["bytes"]}
            publication["release"] = self.add(phase + "/release.json", release)
            head = observer.cli_json(self.root / publication["head"])
            head["payload"].update(latest_release_cid=self.manifest["files"][publication["release"]]["cid"],
                                   release_sha256=self.manifest["files"][publication["release"]]["sha256"])
            publication["head"] = self.add(phase + "/head.json", head)
            publication["receipt"] = self.add(phase + "/receipt.json", {
                "last_head_cid": self.manifest["files"][publication["head"]]["cid"],
                "last_release_cid": self.manifest["files"][publication["release"]]["cid"]}, cid=False)
            for key in ("head", "release"):
                self.manifest["holder"]["content"][self.manifest["files"][publication[key]]["cid"]] = publication[key]
        self.manifest["preserve"]["support"] = ["capsules", *("bin/" + name for name in observer.CLI_NATIVE_SETUP), "fixture-tools"]
        if initial_home:
            self.manifest["initial_home"] = {"entrypoint": "capsules/home/browser/index.html", "files": sorted(mapping)}
        self.manifest["holder"]["content"] = {cid: relative for cid, relative in self.manifest["holder"]["content"].items()
                                              if self.manifest["files"][relative].get("cid") == cid}
        self.freeze()

    def test_setup_archive_and_artifact_closure_are_deterministic_and_admitted(self):
        self.setup_fixture()
        self.admit()
        mapping = observer.cli_admit_setup(self.root, self.manifest)
        archive = self.root / self.manifest["setup"]["artifacts"]["home.tar.gz"]
        self.assertEqual(archive.read_bytes(), observer.cli_home_archive(self.root, self.manifest, mapping))
        self.assertEqual(archive.read_bytes()[4:8], b"\0" * 4)
        with observer.tarfile.open(archive, "r:gz") as package:
            members = package.getmembers()
            expected = {"home/" + target.removeprefix("capsules/home/"): relative
                        for target, relative in mapping.items() if target.startswith("capsules/home/")}
            self.assertEqual([member.name for member in members], sorted(expected))
            for member in members:
                self.assertTrue(member.isfile())
                self.assertEqual((member.uid, member.gid, member.mtime), (0, 0, 0))
                self.assertEqual(package.extractfile(member).read(), (self.root / expected[member.name]).read_bytes())

    def test_setup_admission_refuses_incomplete_or_unmodeled_artifacts(self):
        self.setup_fixture()
        components_path = self.root / self.manifest["publications"]["old"]["components"]
        original = components_path.read_bytes()
        for fault in ("empty profile", "missing artifact", "blank release path", "artifact hash", "capsule metadata", "archive bytes", "holder route"):
            with self.subTest(fault=fault):
                manifest = json.loads(json.dumps(self.manifest))
                components = json.loads(original)
                if fault == "empty profile":
                    components["profiles"]["home"]["components"] = []
                elif fault == "missing artifact":
                    del manifest["setup"]["artifacts"]["kubo"]
                elif fault == "blank release path":
                    components["external"]["kubo"]["platforms"]["darwin-arm64"]["release_path"] = ""
                elif fault == "artifact hash":
                    components["external"]["kubo"]["platforms"]["darwin-arm64"]["checksum"] = "sha256:" + "0" * 64
                elif fault == "capsule metadata":
                    components["external"]["localhost-provider"]["capsule_metadata"] = {"platforms": {}}
                elif fault == "holder route":
                    binding = manifest["files"][manifest["setup"]["artifacts"]["kubo"]]
                    del manifest["holder"]["content"][binding["cid"]]
                else:
                    manifest["setup"]["artifacts"]["home.tar.gz"] = manifest["setup"]["artifacts"]["kubo"]
                observer.write(components_path, components)
                with self.assertRaises(ValueError):
                    observer.cli_admit_setup(self.root, manifest)
        components_path.write_bytes(original)

    def test_generator_requires_qualified_localhost_metadata_before_signing_or_mutation(self):
        support = self.qualified_home_support()
        archive = self.qualified_localhost_metadata(support)
        generated = self.root / "generated"
        runtime = self.root / "retained-runtime"
        runtime.write_bytes(b"prior Runtime remains")
        before = runtime.read_bytes()
        original = archive.read_bytes()
        for fault in ("absent", "tampered", "wrong size"):
            with self.subTest(fault=fault):
                registry = observer.cli_json(support / "components.json")
                registry["external"]["localhost-provider"]["capsule_metadata"]["platforms"]["*"]["size"] = len(original) + int(fault == "wrong size")
                observer.write(support / "components.json", registry)
                archive.write_bytes(original + (b"tampered" if fault == "tampered" else b""))
                with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true"}), \
                     patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
                     patch.object(observer.subprocess, "run") as command, self.assertRaises(ValueError):
                    observer.cli_generate_hop(generated, runtime, runtime, runtime, runtime, support, self.root / "previous",
                                             localhost_metadata=None if fault == "absent" else archive)
                command.assert_not_called()
                self.assertFalse(generated.exists())
                self.assertEqual(runtime.read_bytes(), before)

    def test_localhost_metadata_archive_refuses_traversal_links_modes_and_incomplete_entries(self):
        support = self.qualified_home_support()
        for fault in ("traversal", "link", "hard link", "unsafe mode", "missing manifest", "missing icon",
                      "entry count", "oversized member", "invalid tar"):
            with self.subTest(fault=fault):
                archive = self.qualified_localhost_metadata(support, fault)
                descriptor = observer.cli_json(support / "components.json")["external"]["localhost-provider"]
                selected = observer.cli_localhost_metadata_info(descriptor, "darwin-arm64")
                with self.assertRaises(ValueError):
                    observer.cli_localhost_metadata_files(archive.read_bytes(), selected)

    def test_localhost_metadata_platform_selection_matches_runtime_exact_alias_and_wildcard(self):
        support = self.qualified_home_support()
        self.qualified_localhost_metadata(support)
        descriptor = observer.cli_json(support / "components.json")["external"]["localhost-provider"]
        binding = descriptor["capsule_metadata"]["platforms"]["*"]
        for platform_name, keys, expected in (
                ("darwin-amd64", ("darwin-amd64", "*"), "darwin-amd64"),
                ("darwin-amd64", ("x86_64-darwin", "*"), "*"),
                ("darwin-arm64", ("darwin-arm64", "aarch64-darwin", "*"), "darwin-arm64"),
                ("darwin-arm64", ("aarch64-darwin", "*"), "aarch64-darwin"),
                ("aarch64-darwin", ("darwin-arm64", "*"), "darwin-arm64"),
                ("darwin-arm64", ("*",), "*")):
            with self.subTest(platform=platform_name, keys=keys):
                platforms = {key: {**binding, "note": key} for key in keys}
                descriptor["capsule_metadata"]["platforms"] = platforms
                self.assertIs(observer.cli_localhost_metadata_info(descriptor, platform_name), platforms[expected])
        descriptor["capsule_metadata"]["platforms"] = {"x86_64-darwin": binding}
        with self.assertRaisesRegex(ValueError, "archive binding differs"):
            observer.cli_localhost_metadata_info(descriptor, "darwin-amd64")

    def test_localhost_metadata_setup_admission_requires_artifact_route_and_complete_outputs(self):
        self.setup_fixture(initial_home=True, metadata=True)
        self.admit()
        for fault in ("missing artifact", "missing route", "missing icon", "output hash", "output mode"):
            with self.subTest(fault=fault):
                manifest = json.loads(json.dumps(self.manifest))
                artifact = manifest["setup"]["artifacts"][observer.CLI_LOCALHOST_METADATA]
                icon = "capsules/localhost-provider/icons/icon-256.png"
                if fault == "missing artifact":
                    del manifest["setup"]["artifacts"][observer.CLI_LOCALHOST_METADATA]
                elif fault == "missing route":
                    del manifest["holder"]["content"][manifest["files"][artifact]["cid"]]
                elif fault == "missing icon":
                    del manifest["consumer"]["files"][icon]
                elif fault == "output hash":
                    manifest["files"][manifest["consumer"]["files"][icon]]["sha256"] = "0" * 64
                else:
                    manifest["files"][manifest["consumer"]["files"][icon]]["mode"] = 0o644
                with self.assertRaises(ValueError):
                    observer.cli_admit_setup(self.root, manifest)

    def test_qualified_source_kubo_metadata_is_omitted_from_fixed_setup_fixture(self):
        support = self.qualified_home_support()
        registry = observer.cli_json(support / "components.json")
        registry["external"]["kubo"]["capsule_metadata"] = {"platforms": {"darwin-arm64": {
            "release_path": "kubo-capsule.tar.gz", "install_path": "capsules/kubo"}}}
        observer.write(support / "components.json", registry)
        observer.cli_qualified_home(support, "darwin-arm64")
        self.setup_fixture(support=support)
        components = observer.cli_json(self.root / self.manifest["publications"]["old"]["components"])
        self.assertTrue(all("capsule_metadata" not in components["external"][name]
                            for name in observer.CLI_SETUP_COMPONENTS))
        observer.cli_admit_setup(self.root, self.manifest)

    def test_real_runtime_requires_setup_closure_while_self_tests_still_run_setup(self):
        manifest = json.loads(json.dumps(self.manifest))
        manifest["proof_kind"] = "real-runtime"
        with self.assertRaisesRegex(ValueError, "requires signed setup artifacts"):
            observer.cli_admit_setup(self.root, manifest)
        code, calls, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertTrue(any(argv[1:] == ["setup"] for argv, _ in calls))
        self.assertTrue(all(receipt["status"] == "passed" for receipt in result["setup"].values()))

    def test_runner_setup_precedes_verification_and_every_baseline_seed(self):
        self.setup_fixture(initial_home=True, metadata=True)
        with patch.object(observer, "cli_initial_home", return_value={"status": "passed"}):
            code, calls, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertEqual(len(result["setup"]), 2 + len(observer.CLI_REFUSALS))
        for home in ("cli", "initial-home", *("m2-" + case for case in observer.CLI_REFUSALS)):
            commands = [argv for argv, env in calls if Path(env["HOME"]).name == home]
            install = next(index for index, argv in enumerate(commands) if argv[0] == "/bin/bash")
            setup = next(index for index, argv in enumerate(commands) if argv[1:] == ["setup"])
            version = next(index for index, argv in enumerate(commands) if argv[1:] == ["--version"])
            self.assertLess(install, setup)
            self.assertLess(setup, version)
        self.assertTrue(result["cleanup"]["passed"])
        self.assertEqual(result["transport"]["m2_http_fallback_requests"], 0)
        self.assertFalse(any(argv[1:] == ["home", "--browser"] for argv, _ in calls))

    def test_runner_setup_failure_keeps_receipt_stops_before_update_and_cleans(self):
        self.setup_fixture()
        roots_at_setup = {}
        spawn = observer.CliProcesses.spawn
        def observe(manager, argv, *args):
            if argv[1:] == ["setup"]:
                roots_at_setup.update(manager.roots)
            return spawn(manager, argv, *args)
        with patch.object(observer.CliProcesses, "spawn", observe):
            code, calls, result = self.fake_run(setup_exit=7)
        self.assertEqual(code, 1)
        self.assertIn("installed setup failed; command exit 7", result["failure"])
        self.assertEqual(result["setup"]["m1-setup"]["exit"], 7)
        self.assertTrue(all(str(self.root / "results/homes/cli" / observer.CLI_DATA / ("bin/" + name)) in roots_at_setup
                            for name in observer.CLI_SETUP_COMPONENTS if name not in observer.CLI_WEB_CAPSULES))
        self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_refuses_missing_corrupt_or_unsafe_setup_outputs_before_preservation(self):
        self.setup_fixture(metadata=True)
        original_root = self.root
        for action, relative in (("omit", "components.json"), ("omit", "model-catalog.json"),
                                 ("omit", "bin/kubo"), ("corrupt", "capsules/home/browser/index.html"),
                                 ("mode", "bin/ipfs-provider"), ("mode", "capsules/home/.elastos-cid"),
                                 ("omit", "capsules/localhost-provider/capsule.json"),
                                 ("omit", "capsules/localhost-provider/icons/icon-256.png"),
                                 ("corrupt", "capsules/localhost-provider/.elastos-cid"),
                                 ("corrupt", "capsules/localhost-provider/.elastos-artifact-sha256"),
                                 ("mode", "capsules/localhost-provider/icons/icon-32.png")):
            with self.subTest(action=action, relative=relative):
                code, calls, result = self.fake_run(setup_fault=(action, relative))
                self.assertEqual(code, 1, result)
                self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
                self.assertFalse((self.root / "results/homes/cli" / observer.CLI_DATA / "state/value").exists())
                self.assertTrue(result["cleanup"]["passed"], result["cleanup"])
                self.assertEqual((self.root / "results/homes/cli/.local/bin/elastos").read_bytes(), b"old")
                shutil.rmtree(original_root / "results")

    def test_prepare_preservation_copies_cannot_mask_setup_support(self):
        self.setup_fixture(metadata=True)
        copy = observer.cli_copy
        copied = []
        def observe(root, manifest, mapping, destination):
            if destination == self.root / "results/homes/cli" / observer.CLI_DATA:
                copied.extend(mapping)
                self.assertFalse(set(mapping) & set(observer.cli_setup_files(manifest)))
            return copy(root, manifest, mapping, destination)
        with patch.object(observer, "cli_copy", observe):
            code, _, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertIn("state/value", copied)
        self.assertIn("fixture-tools/open", copied)

    def test_prepare_refuses_setup_support_drift_before_copying_sentinels(self):
        self.setup_fixture()
        verify, checks = observer.cli_verify_setup_support, []
        def observe(root, manifest, home_path):
            if home_path.name == "cli":
                checks.append(home_path)
                if len(checks) == 2:
                    (home_path / observer.CLI_DATA / "capsules/home/browser/index.html").write_bytes(b"drift after setup check")
            return verify(root, manifest, home_path)
        with patch.object(observer, "cli_verify_setup_support", observe):
            code, calls, result = self.fake_run()
        self.assertEqual(code, 1, result)
        self.assertIn("installed setup support differs", result["failure"])
        self.assertFalse((self.root / "results/homes/cli" / observer.CLI_DATA / "state/value").exists())
        self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
        self.assertTrue(result["cleanup"]["passed"], result["cleanup"])

    def initial_home_fixture(self, home_url="http://localhost:8090/home/"):
        support = self.qualified_home_support()
        entry, paths, _ = observer.cli_qualified_home(support, "darwin-arm64")
        mapping = {}
        for path in [*(support / "bin" / name for name in observer.CLI_NATIVE_SETUP), *paths]:
            target = path.relative_to(support).as_posix()
            mapping[target] = self.add("initial/" + target, path.read_bytes(), 0o755 if target.startswith("bin/") else 0o600)
        mapping["fixture-tools/open"] = self.add("initial/opener", b"fixture no-op utility", 0o700)
        self.manifest["consumer"]["files"].update(mapping)
        for relative in mapping.values():
            self.manifest["holder"]["content"][self.manifest["files"][relative]["cid"]] = relative
        self.manifest["initial_home"] = {"entrypoint": "capsules/home/browser/index.html", "files": sorted(mapping)}
        self.manifest["preserve"]["support"] = ["capsules", *("bin/" + name for name in observer.CLI_NATIVE_SETUP), "fixture-tools"]
        components_path = self.root / self.manifest["publications"]["old"]["components"]
        components = observer.cli_json(components_path)
        components["capsules"]["home"] = entry
        qualified = observer.cli_json(support / "components.json")
        components["external"]["home"] = qualified["external"]["home"]
        for name in observer.CLI_NATIVE_SETUP:
            native = self.manifest["files"][mapping["bin/" + name]]
            selected = {"checksum": "sha256:" + native["sha256"], "cid": native["cid"], "size": native["bytes"], "install_path": "bin/" + name}
            components["external"][name] = {**qualified["external"][name], "platforms": {"darwin-arm64": selected}}
        observer.write(components_path, components)
        home = self.root / "initial-home"
        home.mkdir()
        directory = home / observer.CLI_DATA
        directory.mkdir(parents=True)
        observer.cli_copy(self.root, self.manifest, self.manifest["consumer"]["files"], directory)
        binary = home / ".local/bin/elastos"
        binary.parent.mkdir(parents=True)
        binary.write_bytes((self.root / self.manifest["publications"]["old"]["binary"]).read_bytes())
        binary.chmod(0o755)
        for name, key in (("components.json", "components"), ("model-catalog.json", "catalogue")):
            (directory / name).write_bytes((self.root / self.manifest["publications"]["old"][key]).read_bytes())
        sources = {"default_source": "default", "sources": [{"name": "default", "publisher_dids": [self.manifest["signer_did"]],
                   "channel": "canary", "installed_version": self.manifest["old"]["version"], "install_path": str(binary)}]}
        observer.write(directory / "sources.json", sources)
        self.installed_metadata_fixture(home, consumed=False)
        self.installed_metadata_fixture(home)
        key = directory / "identity/device.key"
        key.parent.mkdir()
        key.write_bytes(b"isolated fixture identity")
        controller_directory = directory / "update-controller"
        controller_directory.mkdir()
        controller = controller_directory / "runtime"
        controller.write_bytes(binary.read_bytes())
        controller.chmod(0o700)
        encode = lambda value: base64.b64encode(os.fsencode(value)).decode()
        launch = {"args": [encode("home"), encode("--browser")], "environment": [], "cwd": encode(home)}
        receipt = {"schema": "elastos.update-controller/v1", "data_dir": str(directory), "binary": str(binary), "controller": str(controller),
                   "controller_sha256": observer.digest(binary), "signed_controller_release": base64.b64encode(
                       (self.root / self.manifest["publications"]["old"]["release"]).read_bytes()).decode(),
                   "trusted_source": sources["sources"][0], "launch": launch,
                   "launch_sha256": hashlib.sha256(json.dumps(launch, separators=(",", ":")).encode()).hexdigest()}
        identities = {45001: {"pid": 45001, "parent": os.getpid(), "group": 45001, "start": "macos:100:12"},
                      45002: {"pid": 45002, "parent": 45001, "group": 45002, "start": "macos:101:34"}}
        status = {"id": None, "phase": "ready", "current_version": self.manifest["old"]["version"], "new_version": None,
                  "message": "ready fixture", "controller_pid": 45001, "controller_start": identities[45001]["start"],
                  "host_pid": 45002, "generation": "a" * 32}
        coords = {"api_url": "http://127.0.0.1:60123", "attach_secret": "b" * 64, "pid": 45002, "runtime_kind": "gateway",
                  "binary_sha256": observer.digest(binary), "generation": status["generation"], "home_url": home_url}
        lock = {"pid": 45002, "role": "gateway", "addr": "localhost:8090", "generation": status["generation"]}
        for path, value in ((controller_directory / "receipt.json", receipt), (controller_directory / "status.json", status),
                            (directory / "gateway-runtime-coords.json", coords), (directory / "host-process.lock", lock)):
            observer.write(path, value)
            path.chmod(0o600)
        (controller_directory / "controller.lock").write_bytes(b"")
        (controller_directory / "controller.lock").chmod(0o600)
        process = SimpleNamespace(pid=45001, returncode=None, poll=lambda: None)
        executables = {45001: str(controller), 45002: str(binary)}
        def response(url, limit, payload=None, token=None):
            if url.endswith("/api/auth/attach"):
                self.assertEqual(payload, {"secret": "b" * 64, "scope": "client"})
                return b'{"token":"private fixture token","session_type":"capsule"}'
            if url.endswith("/api/health"):
                self.assertEqual(token, "private fixture token")
                return json.dumps({"version": self.manifest["old"]["version"]}).encode()
            self.assertEqual(url, home_url)
            return (directory / self.manifest["initial_home"]["entrypoint"]).read_bytes()
        return home, directory, process, status, identities, executables, response

    def test_initial_home_matches_signed_controller_live_generation_and_private_attach(self):
        home, directory, process, status, identities, executables, response = self.initial_home_fixture("http://[::1]:8090/home/")
        observer.cli_admit_home_support(self.root, self.manifest)
        manager = SimpleNamespace(roots={})
        with patch.object(observer, "cli_process_identity", side_effect=identities.get), \
             patch.object(observer, "cli_process_executable", side_effect=executables.get), \
             patch.object(observer, "cli_home_response", side_effect=response), patch.object(observer, "lock_state", return_value="held"):
            proof = observer.cli_observe_initial_home(manager, self.manifest, home, process, status)
        self.assertTrue(proof["authenticated_health"])
        self.assertEqual(proof["home_url"], "http://[::1]:8090/home/")
        self.assertEqual(proof["host"], identities[45002])
        self.assertEqual(proof["home_sha256"], observer.digest(directory / self.manifest["initial_home"]["entrypoint"]))
        self.assertNotIn("attach_secret", json.dumps(proof))
        self.assertNotIn("private fixture token", json.dumps(proof))

    def test_initial_home_admission_requires_signed_native_support_and_preservation(self):
        self.initial_home_fixture()
        observer.cli_admit_home_support(self.root, self.manifest)
        for target in ("bin/ipfs-provider", "bin/kubo"):
            for refusal in ("missing", "substituted", "nonexecutable", "unpreserved"):
                with self.subTest(target=target, refusal=refusal):
                    changed = json.loads(json.dumps(self.manifest))
                    if refusal == "missing":
                        del changed["consumer"]["files"][target]
                        changed["initial_home"]["files"].remove(target)
                    elif refusal == "substituted":
                        changed["consumer"]["files"][target] = changed["consumer"]["files"]["bin/localhost-provider"]
                    elif refusal == "nonexecutable":
                        changed["files"][changed["consumer"]["files"][target]]["mode"] = 0o600
                    else:
                        changed["preserve"]["support"].remove(target)
                    with self.assertRaisesRegex(ValueError, "support.*incomplete|signed native Home support"):
                        observer.cli_admit_home_support(self.root, changed)

    def test_home_qualification_and_admission_match_activation_resolution_and_refusals(self):
        self.initial_home_fixture()
        for scope, path in (("qualified", self.root / "support/components.json"),
                            ("signed", self.root / self.manifest["publications"]["old"]["components"])):
            original = path.read_bytes()
            for case, accepted in (("missing", False), ("substituted", False), ("wrong component path", False),
                                   ("wrong selected path", False), ("missing platform", False),
                                   ("alias conflicts with wildcard", False), ("alias precedes wildcard", True),
                                   ("direct precedes alias", True), ("direct refusal precedes alias", False),
                                   ("null path uses component", True), ("empty path", False)):
                with self.subTest(scope=scope, case=case):
                    registry = json.loads(original)
                    if case == "missing":
                        del registry["external"]["home"]
                    elif case == "substituted":
                        registry["external"]["home"] = registry["external"]["localhost-provider"]
                    elif case == "wrong component path":
                        registry["external"]["home"]["install_path"] = "capsules/sibling"
                        del registry["external"]["home"]["platforms"]["*"]["install_path"]
                    elif case == "wrong selected path":
                        registry["external"]["home"]["platforms"]["*"]["install_path"] = "capsules/sibling"
                    elif case == "missing platform":
                        registry["external"]["home"]["platforms"] = {}
                    else:
                        platforms = registry["external"]["home"]["platforms"]
                        if case == "alias conflicts with wildcard":
                            platforms["aarch64-darwin"] = {"install_path": "capsules/sibling"}
                        elif case == "alias precedes wildcard":
                            platforms["aarch64-darwin"] = {"install_path": "capsules/home"}
                            platforms["*"]["install_path"] = "capsules/sibling"
                        elif case == "direct precedes alias":
                            platforms["darwin-arm64"] = {"install_path": "capsules/home"}
                            platforms["aarch64-darwin"] = {"install_path": "capsules/sibling"}
                        elif case == "direct refusal precedes alias":
                            platforms["darwin-arm64"] = {"install_path": "capsules/sibling"}
                            platforms["aarch64-darwin"] = {"install_path": "capsules/home"}
                        else:
                            platforms["*"]["install_path"] = None if case == "null path uses component" else ""
                    observer.write(path, registry)
                    def admit():
                        if scope == "qualified":
                            return observer.cli_qualified_home(self.root / "support", "darwin-arm64")
                        return observer.cli_admit_home_support(self.root, self.manifest)
                    if accepted:
                        admit()
                    else:
                        with self.assertRaisesRegex(ValueError, "Home activation binding"):
                            admit()
            path.write_bytes(original)
        observer.cli_admit_home_support(self.root, self.manifest)

    def test_initial_home_snapshot_preserves_ipfs_provider_and_kubo_bytes(self):
        home, directory, _, _, _, _, _ = self.initial_home_fixture()
        before = observer.cli_home_snapshot(self.manifest, home)
        for target in ("bin/ipfs-provider", "bin/kubo"):
            with self.subTest(target=target):
                path = directory / target
                original = path.read_bytes()
                self.assertEqual(before["preserved"]["support"][target], {".": observer.digest(path)})
                path.write_bytes(b"changed signed support")
                self.assertNotEqual(observer.cli_home_snapshot(self.manifest, home), before)
                path.write_bytes(original)

    def test_initial_home_refuses_substituted_signed_receipt_or_child_generation(self):
        home, directory, process, status, identities, executables, response = self.initial_home_fixture()
        receipt_path = directory / "update-controller/receipt.json"
        receipt = observer.read(receipt_path)
        coords_path = directory / "gateway-runtime-coords.json"
        coords = observer.read(coords_path)
        for refusal in ("signed release", "launch intent", "executable", "parent", "birth", "generation", "served bytes"):
            with self.subTest(refusal=refusal):
                changed_receipt, changed_status, changed_coords = (json.loads(json.dumps(value)) for value in (receipt, status, coords))
                changed_identities = {key: dict(value) for key, value in identities.items()}
                changed_executables = dict(executables)
                if refusal == "signed release":
                    changed_receipt["signed_controller_release"] = base64.b64encode(b"foreign release").decode()
                elif refusal == "launch intent":
                    changed_receipt["launch"]["args"] = [base64.b64encode(b"serve").decode()]
                elif refusal == "executable":
                    changed_executables[45001] = str(home / ".local/bin/elastos")
                elif refusal == "parent":
                    changed_identities[45002]["parent"] = 999
                elif refusal == "birth":
                    changed_status["controller_start"] = "macos:2:3"
                elif refusal == "generation":
                    changed_coords["generation"] = "c" * 32
                observer.write(receipt_path, changed_receipt)
                observer.write(coords_path, changed_coords)
                def reply(url, *args, **kwargs):
                    return b"foreign Home" if refusal == "served bytes" and url.endswith("/home/") else response(url, *args, **kwargs)
                with patch.object(observer, "cli_process_identity", side_effect=changed_identities.get), \
                     patch.object(observer, "cli_process_executable", side_effect=changed_executables.get), \
                     patch.object(observer, "cli_home_response", side_effect=reply), patch.object(observer, "lock_state", return_value="held"), \
                     self.assertRaises(ValueError):
                    observer.cli_observe_initial_home(SimpleNamespace(roots={}), self.manifest, home, process, changed_status)

    def test_qualified_home_refuses_unpinned_or_escaping_support_before_signing(self):
        support = self.qualified_home_support()
        provider = support / "bin/localhost-provider"
        provider.write_bytes(b"substituted provider")
        with self.assertRaisesRegex(ValueError, "provider binding"):
            observer.cli_qualified_home(support, "darwin-arm64")
        provider.write_bytes(b"localhost-provider")
        extra = support / "capsules/home/browser/uninventoried.js"
        extra.write_bytes(b"unqualified script")
        with self.assertRaisesRegex(ValueError, "asset closure"):
            observer.cli_qualified_home(support, "darwin-arm64")
        extra.unlink()
        extra.symlink_to(self.root / "manifest.json")
        with self.assertRaisesRegex(ValueError, "symlink|escapes"):
            observer.cli_qualified_home(support, "darwin-arm64")

    def test_initial_home_owner_stop_preserves_data_and_proves_all_owned_absence(self):
        home, directory, process, status, identities, executables, response = self.initial_home_fixture()
        shutil.rmtree(directory / "installation")
        running = True
        captured = []
        process.poll = lambda: None if running else 0
        def wait(timeout):
            self.assertEqual(timeout, 35)
            process.returncode = 0
            return 0
        process.wait = wait
        def stop(pid, sig):
            nonlocal running
            self.assertEqual((pid, sig), (45001, observer.signal.SIGTERM))
            running = False
            (directory / "gateway-runtime-coords.json").unlink()
        def spawn(argv, env, cwd, label):
            captured.append((argv, env, cwd, label))
            self.installed_metadata_fixture(home)
            return process
        manager = SimpleNamespace(roots={}, spawn=spawn)
        real_observe = observer.cli_observe_initial_home
        def observe(*args):
            with patch.object(observer, "lock_state", return_value="held"):
                return real_observe(*args)
        with patch.object(observer, "cli_process_identity", side_effect=lambda pid: identities.get(pid) if running else None), \
             patch.object(observer, "cli_process_executable", side_effect=executables.get), \
             patch.object(observer, "cli_home_response", side_effect=response), patch.object(observer, "cli_observe_initial_home", side_effect=observe), \
             patch.object(observer, "cli_port_released") as ports, patch.object(observer, "cli_census", return_value=[]), \
             patch.object(observer, "lock_state", return_value="released"), patch.object(observer.os, "killpg", side_effect=stop):
            proof = observer.cli_initial_home(manager, self.manifest, home, {"status": "failed"})
        self.assertEqual(proof["status"], "passed")
        self.assertTrue(proof["cleanup"]["reaped"] and proof["cleanup"]["data_preserved"])
        self.assertTrue(proof["desktop_opener"]["suppressed"])
        self.assertEqual(ports.call_count, 5)
        argv, env, cwd, label = captured[0]
        self.assertEqual(argv[1:], ["home", "--browser"])
        self.assertEqual(env["PATH"].split(":")[0], str(directory / "fixture-tools"))
        self.assertEqual(env["ELASTOS_CARRIER_MDNS"], "0")

    def test_initial_home_refuses_foreign_consumed_authority_before_spawn(self):
        home, directory, _, _, _, _, _ = self.initial_home_fixture()
        self.installed_metadata_fixture(home, "new")
        manager = SimpleNamespace(roots={}, spawn=unittest.mock.Mock())
        with self.assertRaisesRegex(ValueError, "installed metadata differs from signed old release"):
            observer.cli_initial_home(manager, self.manifest, home)
        manager.spawn.assert_not_called()
        shutil.rmtree(directory / "installation")
        (directory / observer.CLI_PUBLISHER / "release.json").write_bytes(b"substituted legacy release")
        with self.assertRaisesRegex(ValueError, "legacy metadata differs from signed old release"):
            observer.cli_initial_home(manager, self.manifest, home)
        manager.spawn.assert_not_called()

    def test_published_home_needs_its_own_disk_reserve_before_it_starts(self):
        # The published release starts Home without its update controller below 15% free;
        # the journey refuses plainly instead of waiting for a controller that never comes.
        home, _, _, _, _, _, _ = self.initial_home_fixture()
        manifest = {**self.manifest, "previous": {}}
        binary = (home / ".local/bin/elastos").stat().st_size
        for free, refused in ((150 * 1024**2, True), (150 * 1024**2 + binary, False)):
            with self.subTest(free=free):
                manager = SimpleNamespace(roots={}, spawn=unittest.mock.Mock(side_effect=RuntimeError("spawned")))
                with patch.object(observer.shutil, "disk_usage", return_value=SimpleNamespace(total=1000 * 1024**2, free=free)), \
                     patch.object(observer, "cli_port_released"), \
                     self.assertRaisesRegex(Exception, "below the published release's 15% reserve" if refused else "spawned"):
                    observer.cli_initial_home(manager, manifest, home)
                self.assertEqual(manager.spawn.called, not refused)
        self.assertEqual(observer.cli_published_reserve(1001), 151)

    def test_initial_home_failure_paths_stop_owner_and_retain_refusals(self):
        home, directory, _, status, identities, _, _ = self.initial_home_fixture()
        coords_path = directory / "gateway-runtime-coords.json"
        coords = observer.read(coords_path)
        sentinel = directory / "state/value"
        original = sentinel.read_bytes()
        for refusal in ("readiness", "readiness nonzero CI", "readiness nonzero operator", "readiness nonzero local", "readiness missing CI log",
                        "owned group", "held lock", "listener", "user data", "live metadata", "shutdown metadata", "publisher"):
            with self.subTest(refusal=refusal):
                running = True
                observer.write(coords_path, coords)
                coords_path.chmod(0o600)
                sentinel.write_bytes(original)
                self.installed_metadata_fixture(home)
                self.installed_metadata_fixture(home, consumed=False)
                process = SimpleNamespace(pid=45001, returncode=None, poll=lambda: None if running else 0)
                def wait(timeout):
                    process.returncode = 1 if "nonzero" in refusal else 0
                    return process.returncode
                process.wait = wait
                def stop(pid, sig):
                    nonlocal running
                    self.assertEqual((pid, sig), (45001, observer.signal.SIGTERM))
                    running = False
                    coords_path.unlink()
                    if refusal == "user data":
                        sentinel.write_bytes(b"changed user data")
                    if refusal == "shutdown metadata":
                        (directory / "installation/release.json").write_bytes(b"changed consumed release")
                    if refusal == "publisher":
                        (directory / observer.CLI_PUBLISHER / "release.json").write_bytes(b"changed publication")
                def observe(*args):
                    if refusal.startswith("readiness"):
                        raise ValueError("injected initial readiness refusal")
                    if refusal == "live metadata":
                        (directory / "installation/release.json").write_bytes(b"changed consumed release")
                    return {"status": "passed", "controller": identities[45001], "host": identities[45002],
                            "home_url": coords["home_url"], "api_url": coords["api_url"]}
                def port(value):
                    if refusal == "listener" and not running:
                        raise ValueError("initial Home listener survives shutdown")
                manager = SimpleNamespace(roots={}, spawn=lambda *args: process)
                manifest = dict(self.manifest)
                evidence = {}
                if "nonzero" in refusal:
                    manager.output = self.root
                    (self.root / "initial-home-start.stdout").write_bytes(b"fixture Home output")
                    (self.root / "initial-home-start.stderr").write_text("x" * 5000 + "\nprivate controller cause")
                    manifest["proof_scope"] = "ci-rehearsal" if refusal.endswith("CI") else "local-rehearsal" if refusal.endswith("local") else "production-positive"
                if "missing" in refusal:
                    manager.output = self.root
                    for stream in ("stdout", "stderr"):
                        (self.root / ("initial-home-start." + stream)).unlink(missing_ok=True)
                with patch.object(observer, "cli_observe_initial_home", side_effect=observe), \
                     patch.object(observer, "cli_process_identity", return_value=None), patch.object(observer, "cli_port_released", side_effect=port), \
                     patch.object(observer, "lock_state", return_value="held" if refusal == "held lock" else "released"), \
                     patch.object(observer, "cli_census", return_value=[{"group": 45002}] if refusal == "owned group" else []), \
                     patch.object(observer.os, "killpg", side_effect=stop) as killed, self.assertRaises(ValueError) as raised:
                    observer.cli_initial_home(manager, manifest, home, evidence)
                self.assertFalse(running)
                self.assertEqual(process.returncode, 1 if "nonzero" in refusal else 0)
                if refusal.startswith("readiness"):
                    self.assertTrue(str(raised.exception).startswith("injected initial readiness refusal"))
                if "nonzero" in refusal:
                    self.assertIn("controller exit 1", str(raised.exception))
                    self.assertEqual("private controller cause" in str(raised.exception), refusal.endswith("CI"))
                    self.assertLess(len(str(raised.exception)), 1200)
                    for stream in ("stdout", "stderr"):
                        self.assertEqual(evidence["controller_" + stream + "_sha256"],
                                         observer.digest(self.root / ("initial-home-start." + stream)))
                if "missing" in refusal:
                    self.assertIn("CI controller diagnostic is unavailable", str(raised.exception))
                killed.assert_called_once()

    def test_initial_home_response_and_private_records_keep_their_bounds(self):
        with patch.object(observer.urllib.request, "build_opener") as opener:
            reply = opener.return_value.open.return_value.__enter__.return_value
            reply.status = 200
            reply.read.return_value = b"too large"
            with self.assertRaisesRegex(ValueError, "exceeds its bound"):
                observer.cli_home_response("http://127.0.0.1:8090/home/", 2)
            reply.read.assert_called_once_with(3)
        path = self.root / "private-home-record"
        path.write_bytes(b'{"pid":1,"pid":2}')
        path.chmod(0o600)
        with self.assertRaisesRegex(ValueError, "repeats a field"):
            observer.cli_private_json(path)
        path.write_bytes(b'{"pid":1}')
        path.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "ownership"):
            observer.cli_private_json(path)
        for value in ("http://localhost:8090/home/", "http://127.0.0.1:8090/home/", "http://[::1]:8090/home/"):
            with self.subTest(accepted=value):
                self.assertEqual(observer.cli_home_base(value, public=True).port, 8090)
        for value in ("https://localhost:8090/home/", "http://example.test:8090/home/", "http://localhost:8090/home/?secret=x",
                      "http://[::1]:8091/home/", "http://[::1]:8090/", "http://[::1]:8090/home/#fragment",
                      "http://owner@[::1]:8090/home/", "http://[::2]:8090/home/"):
            with self.assertRaises(ValueError):
                observer.cli_home_base(value, public=True)

    def test_generator_uses_explicit_disposable_keys_and_real_signature_admission(self):
        self.assert_generated_signed_fixture()

    def test_local_generator_binds_reference_and_keeps_key_support_and_signature_gates(self):
        self.assert_generated_signed_fixture("local-rehearsal:" + "a" * 40 + ":UP-03-test")

    def test_generator_preserves_qualified_localhost_metadata_archive_and_complete_setup_closure(self):
        self.assert_generated_signed_fixture(metadata=True)

    def test_local_build_receipt_is_refused_by_ci_generation_and_keys_are_removed(self):
        self.assert_generated_signed_fixture(foreign_build=True)

    def assert_generated_signed_fixture(self, local_rehearsal=None, foreign_build=False, metadata=False):
        # Fake command delivery exercises generation; existing RFC 8032 test
        # signing supplies valid signatures to the actual installer verifier.
        # This proves observer admission only, and reports harness-self-test.
        native = self.root / "native"
        native.write_bytes(b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16)
        next_runtime = self.root / "next-runtime"
        next_runtime.write_bytes(native.read_bytes() + b"next version")
        system_runtime = self.root / "system-runtime"
        system_runtime.write_bytes(native.read_bytes() + b"System-applied version")
        build_receipt = self.root / "build-receipt.json"
        source = {"commit": "a" * 40, "tree": "a" * 40}
        previous, published_source = self.previous_release(native)
        observer.write(build_receipt, {"schema": "elastos.update-hop.build/v1", "source": source,
            "command": ["cargo", "build", "--locked", "--release", "-p", "elastos-server", "--bin", "elastos"],
            "status": "passed", "cleanup": {"passed": True}, **{name: {"source": binary_source, "version": version,
            "sha256": observer.digest(path), "version_environment": environment} for name, path, version, binary_source, environment in (
                ("old", native, "0.7.1", published_source, None), ("new", next_runtime, "0.7.2", source, "0.7.2"),
                ("next", system_runtime, "0.7.3", source, "0.7.3"))}})
        if local_rehearsal is not None or foreign_build:
            build = observer.cli_json(build_receipt)
            build.update(proof_scope="local-rehearsal", reference=local_rehearsal or "local-rehearsal:" + "a" * 40 + ":UP-03-test")
            observer.write(build_receipt, build)
        support = self.qualified_home_support()
        metadata_path = self.qualified_localhost_metadata(support) if metadata else None
        generated = self.root / "generated"
        real_run, key_paths = observer.subprocess.run, []
        def command(argv, **kwargs):
            versions = {"elastos": b"elastos 0.7.1\n", "elastos-next": b"elastos 0.7.2\n", "elastos-system": b"elastos 0.7.3\n"}
            if Path(argv[0]).name in versions:
                if argv[1] == "--version":
                    return subprocess.CompletedProcess(argv, 0, versions[Path(argv[0]).name], b"")
                self.assertEqual(argv[1], "sign-payload")
                key = Path(argv[argv.index("--key") + 1])
                key_paths.append(key)
                self.assertEqual(key.stat().st_mode & 0o777, 0o600)
                signed = signing_fixture.sign_envelope(json.loads(kwargs["input"]), argv[argv.index("--domain") + 1], bytes.fromhex(key.read_text()))
                return subprocess.CompletedProcess(argv, 0, json.dumps({field: signed[field] for field in ("signature", "signer_did")}).encode(), b"")
            if Path(argv[0]).name == "kubo":
                repo = Path(kwargs["env"]["IPFS_PATH"])
                if argv[1] == "init":
                    (repo / "blocks").mkdir(parents=True)
                    return subprocess.CompletedProcess(argv, 0, b"", b"")
                raw = Path(argv[-1]).read_bytes()
                codec, block = 0x55, raw
                if "--raw-leaves=false" in argv:
                    def varint(value):
                        result = bytearray()
                        while value >= 128:
                            result.append((value & 127) | 128)
                            value >>= 7
                        return bytes(result + bytes([value]))
                    unixfs = b"\x08\x02\x12" + varint(len(raw)) + raw + b"\x18" + varint(len(raw))
                    codec, block = 0x70, b"\x0a" + varint(len(unixfs)) + unixfs
                cid = "b" + base64.b32encode(bytes([1, codec, 0x12, 0x20]) + hashlib.sha256(block).digest()).decode().lower().rstrip("=")
                (repo / "blocks" / cid).write_bytes(block)
                return subprocess.CompletedProcess(argv, 0, (cid + "\n").encode(), b"")
            return real_run(argv, **kwargs)
        environment = {"CI": "true", "GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}
        if local_rehearsal is not None:
            environment.update(CI="", GITHUB_ACTIONS="", ELASTOS_CI_FIXTURE_REFERENCE="", ELASTOS_CI_FIXTURE_SCOPE="")
        def git(argv, **kwargs):
            return "" if argv[1] == "status" else "a" * 40 + "\n"
        with patch.dict(observer.os.environ, environment), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
             patch.object(observer.subprocess, "run", side_effect=command), patch.object(observer.subprocess, "check_output", side_effect=git):
            if foreign_build:
                with self.assertRaisesRegex(ValueError, "hop build proof scope differs"):
                    observer.cli_generate_hop(generated, native, next_runtime, system_runtime, build_receipt, support, previous)
                self.assertTrue(key_paths)
                self.assertTrue(all(not path.exists() for path in key_paths))
                self.assertFalse((generated / "generator").exists())
                return
            receipt = observer.cli_generate_hop(generated, native, next_runtime, system_runtime, build_receipt, support, previous,
                                                local_rehearsal, metadata_path)
        self.assertEqual(receipt["proof_scope"], "local-rehearsal" if local_rehearsal is not None else "ci-rehearsal")
        if local_rehearsal is not None:
            self.assertEqual(receipt["reference"], local_rehearsal)
        self.assertTrue(receipt["keys_removed"])
        self.assertTrue(key_paths)
        self.assertTrue(all(not path.exists() for path in key_paths))
        self.assertFalse((generated / "generator").exists())
        generated_manifest = observer.cli_json(generated / "manifest.json")
        observer.cli_admit_model_fixture(generated, generated_manifest)
        model_fixture = generated_manifest["model_fixture"]
        if local_rehearsal is not None:
            self.assertTrue((generated / model_fixture["files"]["PROVENANCE.md"]).read_text().startswith("Owned local rehearsal fixture"))
            config_path = generated / generated_manifest["consumer"]["files"]["config/fixture.json"]
            self.assertEqual(observer.cli_json(config_path)["owner"], "isolated local rehearsal")
        catalogue_path = generated / generated_manifest["publications"]["old"]["catalogue"]
        original_catalogue = catalogue_path.read_bytes()
        for refusal in ("empty entries", "missing publication time", "closure digest", "missing package file", "holder CID"):
            with self.subTest(model_refusal=refusal):
                changed = json.loads(json.dumps(generated_manifest))
                catalogue = json.loads(original_catalogue)
                if refusal == "empty entries":
                    catalogue["payload"]["entries"] = []
                elif refusal == "missing publication time":
                    del catalogue["payload"]["published_at"]
                elif refusal == "closure digest":
                    catalogue["payload"]["entries"][0]["object_manifest"]["content_digest"] = "sha256:" + "0" * 64
                elif refusal == "missing package file":
                    del changed["model_fixture"]["files"]["LICENSE"]
                else:
                    cid = changed["files"][model_fixture["package"]]["cid"]
                    del changed["holder"]["content"][cid]
                observer.write(catalogue_path, catalogue)
                with self.assertRaises(ValueError):
                    observer.cli_admit_model_fixture(generated, changed)
        catalogue_path.write_bytes(original_catalogue)
        self.assertEqual(set(generated_manifest["publications"]), {*observer.CLI_PHASES, observer.CLI_SYSTEM_PHASE})
        components = observer.cli_json(generated / generated_manifest["publications"]["new"]["components"])
        qualified = observer.cli_json(support / "components.json")
        if metadata:
            selected = observer.cli_localhost_metadata_info(components["external"]["localhost-provider"], "darwin-arm64")
            original = observer.cli_localhost_metadata_info(qualified["external"]["localhost-provider"], "darwin-arm64")
            self.assertEqual({key: value for key, value in selected.items() if key != "cid"}, original)
            relative = generated_manifest["setup"]["artifacts"][observer.CLI_LOCALHOST_METADATA]
            self.assertEqual((generated / relative).read_bytes(), metadata_path.read_bytes())
            expected = observer.cli_localhost_metadata_files(metadata_path.read_bytes(), original)
            self.assertEqual(generated_manifest["setup"]["localhost_metadata"]["files"], sorted(expected))
            self.assertEqual({target: generated_manifest["files"][generated_manifest["consumer"]["files"][target]]["mode"]
                              for target in expected}, {target: 0o600 for target in expected})
        consumer_data = generated / "copied-consumer" / observer.CLI_DATA
        observer.cli_copy(generated, generated_manifest, generated_manifest["consumer"]["files"], consumer_data)
        self.assertEqual(components["external"]["home"]["platforms"]["darwin-arm64"]["release_path"], "home.tar.gz")
        # Published like a release: an empty capsules map, so `update` cannot evict a refreshed capsule.
        self.assertEqual(components["capsules"], {})
        entries = observer.cli_capsule_entries(generated, generated_manifest, components)
        home_activation = observer.cli_home_activation_descriptor(components, "darwin-arm64")
        active_home = consumer_data / home_activation["platforms"]["darwin-arm64"]["install_path"]
        self.assertTrue(active_home.is_dir())
        copied_home = observer.cli_json(active_home / "capsule.json")
        self.assertEqual(copied_home["name"], "home")
        self.assertEqual(observer.digest(active_home / copied_home["entrypoint"]),
                         entries["home"]["entrypoint_sha256"].removeprefix("sha256:"))
        for name in ("ipfs-provider", "kubo", "localhost-provider"):
            target = "bin/" + name
            relative = generated_manifest["consumer"]["files"][target]
            binding = generated_manifest["files"][relative]
            if name != "localhost-provider":
                self.assertEqual(generated_manifest["holder"]["files"][observer.CLI_DATA + "/" + target], relative)
            self.assertEqual(observer.digest(consumer_data / target), binding["sha256"])
            self.assertTrue((consumer_data / target).stat().st_mode & 0o111)
            self.assertIn(target, observer.cli_setup_files(generated_manifest))
            selected = components["external"][name]["platforms"]["darwin-arm64"]
            self.assertEqual(selected["checksum"], "sha256:" + binding["sha256"])
            self.assertEqual(selected["size"], binding["bytes"])
            self.assertEqual(selected["cid"], binding["cid"])
            if "provider_runtime" in qualified["external"][name]:
                self.assertEqual(components["external"][name]["provider_runtime"], qualified["external"][name]["provider_runtime"])
        package_fields = {"cid", "sha256", "size"}
        self.assertEqual({key: value for key, value in entries["home"].items() if key not in package_fields},
                         {key: value for key, value in qualified["capsules"]["home"].items() if key not in package_fields})
        for view in observer.cli_support_views(generated_manifest):
            observer.cli_admit_setup(generated, view)
        self.assertEqual(components["profiles"]["home"]["components"], list(observer.CLI_SETUP_COMPONENTS))
        # A release lists custody outside the Home profile; Home must start without it (#238).
        custody = components["external"]["custody-provider"]["platforms"]["darwin-arm64"]
        self.assertNotIn("custody-provider", components["profiles"]["home"]["components"])
        self.assertEqual(custody["checksum"], "sha256:" + observer.digest(support / "bin/custody-provider"))
        self.assertNotIn(custody["cid"], generated_manifest["holder"]["content"])
        self.assertNotIn("bin/custody-provider", generated_manifest["consumer"]["files"])
        old_view = observer.cli_support_view(generated_manifest, "old")
        self.assertTrue(all(generated_manifest["files"][old_view["consumer"]["files"][target]].get("cid")
                            for target in generated_manifest["initial_home"]["files"]))
        native = generated_manifest["files"][generated_manifest["consumer"]["files"]["bin/localhost-provider"]]
        self.assertEqual(components["external"]["localhost-provider"]["platforms"]["darwin-arm64"]["cid"], native["cid"])
        # Old is the published release: its Runtime, source, support bytes and installer source.
        publications = generated_manifest["publications"]
        old_components = observer.cli_json(generated / publications["old"]["components"])
        for name in ("ipfs-provider", "localhost-provider", *observer.CLI_PROTECTED_PROVIDERS):
            self.assertEqual(old_components["external"][name]["platforms"]["darwin-arm64"]["checksum"],
                             "sha256:" + observer.digest(previous / "support/bin" / name))
            self.assertNotEqual(old_components["external"][name], components["external"][name])
        self.assertEqual(old_view["consumer"]["files"]["capsules/home/browser/index.html"],
                         generated_manifest["previous"]["support"]["capsules/home/browser/index.html"])
        self.assertEqual(generated_manifest["old"]["source"], published_source)
        self.assertEqual(observer.cli_json(generated / publications["old"]["release"])["payload"]["source"], published_source)
        self.assertEqual(generated_manifest["files"][publications["old"]["binary"]]["sha256"], observer.digest(previous / "elastos"))
        self.assertEqual(observer.cli_installer_metadata(generated / generated_manifest["previous"]["installer"], {}),
                         [generated_manifest["signer_did"], ""])
        # Refusals claim the release after new, so an installed new release must refuse them.
        self.assertEqual({observer.cli_json(generated / publications[case]["release"])["payload"]["version"]
                          for case in observer.CLI_REFUSALS}, {"0.7.3"})
        self.assertNotEqual(generated_manifest["signer_did"], observer.cli_json(generated / generated_manifest["publications"]["wrong-signer-head"]["head"])["signer_did"])
        generated_manifest["proof_kind"] = "harness-self-test"
        observer.write(generated / "manifest.json", generated_manifest)
        config = observer.cli_json(generated / "fixture.json")
        config["immutable"]["sha256"] = observer.digest(generated / "manifest.json")
        admission_environment = {"ELASTOS_CI_FIXTURE_MANIFEST_SHA256": config["immutable"]["sha256"], "ELASTOS_CI_FIXTURE_REFERENCE": ""}
        if local_rehearsal is not None:
            admission_environment.update(CI="", GITHUB_ACTIONS="", ELASTOS_CI_FIXTURE_SCOPE="")
        with patch.dict(observer.os.environ, admission_environment), patch.object(observer.sys, "platform", "darwin"), \
             patch.object(observer.subprocess, "check_output", side_effect=git):
            self.assertEqual(observer.cli_admit(config, local_rehearsal)["proof_kind"], "harness-self-test")
        return generated, generated_manifest

    def test_cross_version_admission_requires_the_pinned_release_on_the_old_side(self):
        generated, manifest = self.assert_generated_signed_fixture()
        observer.cli_admit_previous(generated, manifest, {})
        def same_build_old(changed):
            changed["publications"]["old"]["binary"] = changed["publications"]["new"]["binary"]
        def same_source_old(changed):
            changed["old"]["source"] = changed["source"]
        def new_support_as_old(changed):
            changed["previous"]["support"]["bin/ipfs-provider"] = changed["consumer"]["files"]["bin/ipfs-provider"]
        def skipped_release(changed):
            changed["new"]["version"] = "0.7.3"
        def new_components_as_old(changed):
            changed["publications"]["old"]["components"] = changed["publications"]["new"]["components"]
        def new_build_as_system_release(changed):
            changed["publications"]["next"]["binary"] = changed["publications"]["new"]["binary"]
        def system_release_skips_one(changed):
            changed["next"]["version"] = "0.7.4"
        for change in (same_build_old, same_source_old, new_support_as_old, skipped_release, new_components_as_old,
                       new_build_as_system_release, system_release_skips_one):
            with self.subTest(change=change.__name__):
                changed = json.loads(json.dumps(manifest))
                change(changed)
                with self.assertRaises(ValueError):
                    observer.cli_admit_previous(generated, changed, {})

    def test_next_release_advances_the_last_prerelease_field_or_the_patch(self):
        for version, expected in (("0.8.0-alpha.6", "0.8.0-alpha.7"), ("0.7.1", "0.7.2"), ("1.2.3-rc.9", "1.2.3-rc.10")):
            self.assertEqual(observer.cli_next_version(version), expected)
        for version in ("0.8.0-dev", "0.8", "v0.8.0"):
            with self.subTest(version=version), self.assertRaises(ValueError):
                observer.cli_next_version(version)

    def test_previous_release_admits_only_the_maintainer_signed_bytes_the_pin_names(self):
        runtime = self.root / "runtime"
        runtime.write_bytes(b"published Runtime")
        previous, source = self.previous_release(runtime, "0.8.0-alpha.6")
        pin = observer.cli_previous_pin()
        self.assertEqual(observer.cli_previous_release(previous / "release.json", pin)["source"], source)
        tampered = observer.cli_json(previous / "release.json")
        tampered["payload"]["version"] = "0.8.0-alpha.7"
        observer.write(self.root / "tampered-release.json", tampered)
        for fault, path, changes in (("bytes", self.root / "tampered-release.json", {}),
                                     ("signer", previous / "release.json", {"signer_did": public_did(b"\x01" * 32)}),
                                     ("version", previous / "release.json", {"version": "0.8.0-alpha.5"})):
            with self.subTest(fault=fault), self.assertRaises(ValueError):
                observer.cli_previous_release(path, {**pin, **changes})

    def test_previous_release_fetch_refuses_bytes_that_differ_from_their_signed_hash(self):
        runtime = self.root / "runtime"
        runtime.write_bytes(b"published Runtime")
        previous, _ = self.previous_release(runtime)
        pin = observer.cli_previous_pin()
        release = "/ipfs/" + pin["release_cid"]
        served = {release: (previous / "release.json").read_bytes(), "/ipfs/Qmbinary": b"substituted Runtime bytes"}
        reads = []
        def origin(path, limit):
            reads.append(path)
            return served[path]
        cache = self.root / "cache"
        with patch.object(observer, "cli_origin_bytes", side_effect=origin), patch.object(observer.platform, "machine", return_value="arm64"):
            for attempt in ("first", "cached"):
                with self.subTest(attempt=attempt), self.assertRaisesRegex(ValueError, "elastos differs from its signed hash"):
                    observer.cli_fetch_previous_release(cache, self.root / ("fetched-" + attempt))
        # Only the signed release metadata was cached, and the second fetch read it from the cache.
        self.assertEqual([path.name for path in cache.iterdir()], [pin["release_sha256"]])
        self.assertEqual(reads, [release, "/ipfs/Qmbinary", "/ipfs/Qmbinary"])

    def test_check_result_requires_the_published_start_no_new_provider_warnings_and_undo(self):
        def result():
            return {"schema": "elastos.update-hop.result/v1", "status": "passed",
                    "old": {"version": "0.8.0-alpha.6"}, "new": {"version": "0.8.0-alpha.7"},
                    "previous_release": {"version": "0.8.0-alpha.6"},
                    "paths": {"m1-install": {"checks": {"initial-home": {
                        "system": {"provider_warnings": {"new": []}, "frozen_support": {"phase": "restored"},
                                   "cli_update": {"status": "passed"}},
                        "undo": {"status": "passed", "version": "0.8.0-alpha.6",
                                 "refused_plain_update": {"exit": 1}, "rollback": {"exit": 0}}}}}}}
        self.assertEqual(observer.cli_check_result(result())["from"], "0.8.0-alpha.6")
        home = lambda value: value["paths"]["m1-install"]["checks"]["initial-home"]  # noqa: E731
        faults = {"failed run": lambda value: value.update(status="failed", failure="System did not update"),
                  "new provider warning": lambda value: home(value)["system"]["provider_warnings"].update(new=["provider refused"]),
                  "same-build hop": lambda value: value.pop("previous_release"),
                  "other old release": lambda value: value["old"].update(version="0.8.0-alpha.5"),
                  "no Undo": lambda value: home(value).pop("undo"),
                  # alpha.6's System applying changed support would hide the known limit; record it.
                  "published System applied changed support": lambda value: home(value)["system"]["frozen_support"].update(phase="updated"),
                  "no CLI update from the published release": lambda value: home(value)["system"].pop("cli_update"),
                  "plain downgrade applied": lambda value: home(value)["undo"]["refused_plain_update"].update(exit=0)}
        for name, fault in faults.items():
            with self.subTest(fault=name), self.assertRaises(ValueError):
                value = result()
                fault(value)
                observer.cli_check_result(value)

    def test_generator_refuses_operator_or_non_ci_signing(self):
        with patch.dict(observer.os.environ, {"CI": "false", "GITHUB_ACTIONS": "false"}), self.assertRaisesRegex(ValueError, "native Mac CI"):
            observer.cli_generate_hop(self.root / "generated", *(self.root / "missing" for _ in range(4)), self.root, self.root / "missing")

    def test_hop_builder_preserves_n_and_compiles_n_plus_one_with_version_input(self):
        self.assert_hop_builder()

    def test_local_hop_builder_binds_clean_source_reference_and_native_versions(self):
        self.assert_hop_builder("local-rehearsal:" + "a" * 40 + ":UP-03-test")

    def assert_hop_builder(self, local_rehearsal=None):
        header = b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16
        published = self.root / "published-runtime"
        published.write_bytes(header + b"published")
        previous, published_source = self.previous_release(published, "0.8.0-alpha.6")
        runtime = self.root / "built-runtime"
        runtime.write_bytes(header + b"N")
        destination = self.root / "build-inputs"
        calls = []
        def command(manager, argv, env, cwd, label, timeout):
            calls.append((argv, env, timeout))
            stdout = b""
            if label == "old-version":
                self.assertEqual(argv[0], str(destination / "elastos-old"))
                stdout = b"elastos 0.8.0-alpha.6\n"
            elif label in ("build-next", "build-system"):
                version = "0.8.0-alpha.7" if label == "build-next" else "0.8.0-alpha.8"
                self.assertEqual(env["ELASTOS_RELEASE_VERSION"], version)
                self.assertEqual(env, {**os.environ, "ELASTOS_RELEASE_VERSION": version})
                self.assertEqual(timeout, 900)
                runtime.write_bytes(header + (b"N+1" if label == "build-next" else b"N+2"))
            else:
                stdout = b"elastos 0.8.0-alpha.7\n" if label == "new-version" else b"elastos 0.8.0-alpha.8\n"
            (destination / (label + ".stdout")).write_bytes(stdout)
            (destination / (label + ".stderr")).write_bytes(b"")
            return {"exit": 0}
        def git(argv, **kwargs):
            return "" if argv[1] == "status" else "a" * 40 + "\n"
        environment = {"CI": "true", "GITHUB_ACTIONS": "true",
                       "CARGO_TARGET_DIR": str(self.root / "fresh-target"),
                       "CARGO_BUILD_BUILD_DIR": str(self.root / "fresh-build"),
                       "CARGO_PROFILE_RELEASE_LTO": "false", "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
                       "RUSTFLAGS": "-D warnings", "ELASTOS_RELEASE_VERSION": "0.7.1"}
        if local_rehearsal is not None:
            environment.update(CI="", GITHUB_ACTIONS="", ELASTOS_CI_FIXTURE_REFERENCE="", ELASTOS_CI_FIXTURE_SCOPE="")
        with patch.dict(observer.os.environ, environment), patch.object(observer.sys, "platform", "darwin"), \
             patch.object(observer.platform, "machine", return_value="arm64"), patch.object(observer.subprocess, "check_output", side_effect=git), \
             patch.object(observer.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * 1024**3, free=50 * 1024**3)), \
             patch.object(observer.CliProcesses, "command", command), patch.object(observer.CliProcesses, "cleanup", return_value={"errors": []}):
            receipt = observer.cli_build_hop(destination, runtime, previous, local_rehearsal)
        self.assertEqual(receipt["status"], "passed")
        # Old is the published release, not this source's build.
        self.assertEqual((destination / "elastos-old").read_bytes(), header + b"published")
        self.assertEqual((receipt["old"]["version"], receipt["old"]["source"]), ("0.8.0-alpha.6", published_source))
        self.assertEqual((destination / "elastos-new").read_bytes(), header + b"N+1")
        self.assertEqual(receipt["new"]["version_environment"], "0.8.0-alpha.7")
        self.assertEqual(receipt["new"]["sha256"], observer.digest(destination / "elastos-new"))
        # The release new's System applies is this source compiled again as the one after it.
        self.assertEqual((destination / "elastos-system").read_bytes(), header + b"N+2")
        self.assertEqual((receipt["next"]["version"], receipt["next"]["source"]), ("0.8.0-alpha.8", receipt["new"]["source"]))
        self.assertEqual(receipt["next"]["sha256"], observer.digest(destination / "elastos-system"))
        self.assertEqual([argv for argv, _, _ in calls if argv == receipt["command"]], [receipt["command"]] * 2)
        if local_rehearsal is not None:
            self.assertEqual((receipt["proof_scope"], receipt["reference"]), ("local-rehearsal", local_rehearsal))

    def test_hop_builder_refuses_a_published_runtime_that_differs_from_its_pin_or_this_mac(self):
        header = b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16
        published = self.root / "published-runtime"
        published.write_bytes(header + b"published")
        previous, _ = self.previous_release(published)
        for machine, substituted, message in (("arm64", True, "previous release Runtime differs from its signed hash"),
                                              ("x86_64", False, "previous release has no build for this Mac")):
            with self.subTest(machine=machine):
                (previous / "elastos").write_bytes(header + (b"substituted" if substituted else b"published"))
                with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true"}), patch.object(observer.sys, "platform", "darwin"), \
                     patch.object(observer.platform, "machine", return_value=machine), patch.object(observer.CliProcesses, "command") as command, \
                     patch.object(observer.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * 1024**3, free=50 * 1024**3)), \
                     self.assertRaisesRegex(ValueError, message):
                    observer.cli_build_hop(self.root / "build-inputs", published, previous)
                command.assert_not_called()
                self.assertFalse((self.root / "build-inputs").exists())

    def test_hop_builder_failed_compile_keeps_old_bytes_and_failed_cleanup_receipt(self):
        runtime = self.root / "built-runtime"
        original = b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16
        runtime.write_bytes(original)
        previous, _ = self.previous_release(runtime)
        destination = self.root / "build-inputs"
        def command(manager, argv, env, cwd, label, timeout):
            (destination / (label + ".stdout")).write_bytes(b"elastos 0.7.1\n" if label == "old-version" else b"")
            (destination / (label + ".stderr")).write_bytes(b"")
            return {"exit": 0 if label == "old-version" else 124}
        def git(argv, **kwargs):
            return "" if argv[1] == "status" else "a" * 40 + "\n"
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true"}), patch.object(observer.sys, "platform", "darwin"), \
             patch.object(observer.platform, "machine", return_value="arm64"), patch.object(observer.subprocess, "check_output", side_effect=git), \
             patch.object(observer.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * 1024**3, free=50 * 1024**3)), \
             patch.object(observer.CliProcesses, "command", command), patch.object(observer.CliProcesses, "cleanup", return_value={"errors": []}), \
             self.assertRaisesRegex(ValueError, "new Runtime build failed"):
            observer.cli_build_hop(destination, runtime, previous)
        receipt = observer.cli_json(destination / "build.json")
        self.assertEqual(receipt["status"], "failed")
        self.assertTrue(receipt["cleanup"]["passed"])
        self.assertEqual((destination / "elastos-old").read_bytes(), original)
        self.assertFalse((destination / "elastos-new").exists())

    def test_hop_builder_low_space_refuses_before_commands_or_mutation(self):
        runtime = self.root / "built-runtime"
        original = b"retained old Runtime bytes"
        runtime.write_bytes(original)
        previous, _ = self.previous_release(runtime)
        before = runtime.stat()
        paths = set(self.root.iterdir())
        destination = self.root / "build-inputs"
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true"}), patch.object(observer.sys, "platform", "darwin"), \
             patch.object(observer.shutil, "disk_usage", return_value=SimpleNamespace(total=100 * 1024**3, free=4 * 1024**3 - 1)), \
             patch.object(observer.subprocess, "check_output") as git, patch.object(observer.CliProcesses, "command") as command, \
             patch.object(observer.CliProcesses, "cleanup") as cleanup, patch.object(observer.shutil, "copyfile") as copy, \
             patch.object(observer.Path, "mkdir") as mkdir, self.assertRaisesRegex(ValueError, "hop rebuild needs more free disk space"):
            observer.cli_build_hop(destination, runtime, previous)
        for action in (git, command, cleanup, copy, mkdir):
            action.assert_not_called()
        self.assertFalse(destination.exists())
        self.assertEqual(set(self.root.iterdir()), paths)
        self.assertEqual(runtime.read_bytes(), original)
        self.assertEqual((runtime.stat().st_ino, runtime.stat().st_mode), (before.st_ino, before.st_mode))

    def test_generator_command_failure_removes_keys_and_repository(self):
        support = self.qualified_home_support()
        runtime = self.root / "runtime"
        runtime.write_bytes(b"Runtime command substitute")
        previous, _ = self.previous_release(runtime)
        generated = self.root / "generated"
        real_run = observer.subprocess.run
        def failed(argv, **kwargs):
            if argv[0] == "/bin/bash":
                return real_run(argv, **kwargs)
            self.assertTrue((generated / "generator/approved.key").is_file())
            self.assertTrue((generated / "generator/other.key").is_file())
            return subprocess.CompletedProcess(argv, 1, b"", b"private command error stays out of receipts")
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
             patch.object(observer.subprocess, "run", side_effect=failed), patch.object(observer.subprocess, "check_output", return_value="a" * 40 + "\n"), \
             self.assertRaisesRegex(ValueError, "disposable fixture command failed"):
            observer.cli_generate_hop(generated, runtime, runtime, runtime, runtime, support, previous)
        self.assertFalse((generated / "generator").exists())

    def fake_run(self, apply_stderr="", local_did=HOLDER_DID, bootstrap_fields=None, restart_fields=None,
                 cleanup_error=False, holder_stderr="", holder_shutdown_stderr="", http_fallback=False,
                 config_drift=False, user_data_drift=False, coordination_pid_drift=False, metadata_drift=None,
                 apply_exit=0, local_rehearsal=None, setup_exit=0, setup_fault=None):
        manifest, root, calls, processes = self.manifest, self.root, [], []
        self_test = self
        holder_data = root / "results/homes/holder" / observer.CLI_DATA
        bootstrap_calls = 0
        initial_holder_config = None
        def phase():
            actual = observer.digest(holder_data / observer.CLI_PUBLISHER / "release-head.json")
            return next(name for name, pub in manifest["publications"].items() if manifest["files"][pub["head"]]["sha256"] == actual)
        def copy_publication(home_path, name, first=False, gateway=""):
            pub = manifest["publications"][name]
            directory = home_path / observer.CLI_DATA
            directory.mkdir(parents=True, exist_ok=True)
            for key, target in (("binary", home_path / ".local/bin/elastos"), ("components", directory / "components.json"), ("catalogue", directory / "model-catalog.json")):
                if first and key != "binary":
                    continue
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((root / pub[key]).read_bytes())
            if first:
                self.installed_metadata_fixture(home_path, name, consumed="setup" in manifest)
                if metadata_drift == "legacy":
                    (directory / observer.CLI_PUBLISHER / "release.json").write_bytes(b"substituted legacy release")
                source = {"name": "default", "publisher_dids": [manifest["signer_did"]], "channel": "canary", "connect_ticket": public_ticket(),
                          "publisher_node_id": HOLDER_NODE, "install_path": str(home_path / ".local/bin/elastos"), "installed_version": manifest[name]["version"], "head_cid": "", "gateways": [gateway]}
                observer.write(directory / "sources.json", {"default_source": "default", "sources": [source]})
            else:
                self.installed_metadata_fixture(home_path, name)
                if metadata_drift == "consumed":
                    (directory / "installation/release.json").write_bytes(b"substituted consumed release")
                if metadata_drift == "publisher":
                    (directory / observer.CLI_PUBLISHER / "release.json").write_bytes(b"changed publication")
                sources = observer.read(directory / "sources.json")
                sources["sources"][0].update(installed_version=manifest[name]["version"], head_cid=manifest["files"][pub["head"]]["cid"])
                if metadata_drift == "head CID":
                    sources["sources"][0]["head_cid"] = manifest["files"][manifest["publications"]["old"]["head"]]["cid"]
                observer.write(directory / "sources.json", sources)
        class Process:
            def __init__(self, argv, **kwargs):
                nonlocal initial_holder_config
                self.argv, self.home = argv, Path(kwargs["cwd"])
                self.stderr = kwargs["stderr"]
                self.pid, self.returncode = 20000 + len(processes), 0
                calls.append((argv, kwargs["env"]))
                processes.append(self)
                stdout, stderr = "", ""
                args = argv[1:]
                if args[0] == "gateway":
                    config = (holder_data / "config.toml").read_text()
                    # Model the actual public-bootstrap gate; mocks cannot supply it for free.
                    if 'gateway_public_publisher_bootstrap = true\n' not in config:
                        raise ValueError("public publisher bootstrap is disabled")
                    if initial_holder_config is None:
                        initial_holder_config = config
                    elif config != initial_holder_config:
                        raise ValueError("holder transport configuration changed")
                    self.returncode = None
                    stderr = holder_stderr
                    if config_drift:
                        (holder_data / "config.toml").write_text(config + "# changed\n")
                elif args[:2] == ["node", "info"]:
                    key = self.home / observer.CLI_DATA / "identity/device.key"
                    key.parent.mkdir(parents=True, exist_ok=True)
                    key.write_bytes(b"observer fake identity bytes only")
                    stdout = json.dumps({"did": local_did if self.home.name == "holder" else "did:key:consumer"})
                elif argv[0].endswith("/kubo"):
                    Path(kwargs["env"]["IPFS_PATH"]).mkdir(parents=True)
                elif args == ["setup"]:
                    directory = self.home / observer.CLI_DATA
                    self_test.assertFalse((directory / "components.json").exists())
                    self_test.assertFalse((directory / "model-catalog.json").exists())
                    stderr = "ElastOS fixture — setup for darwin-arm64\n"
                    if setup_exit:
                        self.returncode = setup_exit
                    else:
                        publication = manifest["publications"][phase()]
                        for filename, key in (("components.json", "components"), ("model-catalog.json", "catalogue")):
                            (directory / filename).write_bytes((root / publication[key]).read_bytes())
                        if "setup" in manifest:
                            observer.cli_admit_setup(root, manifest)
                            for artifact in manifest["setup"]["artifacts"]:
                                source = holder_data / observer.CLI_PUBLISHER / "artifacts" / artifact
                                if artifact.endswith(".tar.gz"):
                                    with observer.tarfile.open(fileobj=io.BytesIO(source.read_bytes()), mode="r:gz") as archive:
                                        for member in archive.getmembers():
                                            if member.isdir():
                                                continue
                                            target = directory / "capsules" / member.name
                                            target.parent.mkdir(parents=True, exist_ok=True)
                                            target.write_bytes(archive.extractfile(member).read())
                                            target.chmod(member.mode & ~0o077)
                                else:
                                    target = directory / "bin" / artifact
                                    target.parent.mkdir(parents=True, exist_ok=True)
                                    target.write_bytes(source.read_bytes())
                                    target.chmod(0o755)
                            capsules = observer.cli_json(directory / "components.json")["capsules"]
                            caches = [("capsules/" + name, (capsules[name]["cid"], capsules[name]["sha256"]))
                                      for name in observer.CLI_WEB_CAPSULES]
                            if "localhost_metadata" in manifest["setup"]:
                                components = observer.cli_json(directory / "components.json")
                                selected = observer.cli_localhost_metadata_info(components["external"]["localhost-provider"], "darwin-arm64")
                                caches.append(("capsules/localhost-provider", (selected["cid"], selected["checksum"])))
                            for folder, values in caches:
                                for filename, value in zip(observer.CLI_HOME_CACHE, values):
                                    target = directory / folder / filename
                                    target.write_text(value + "\n")
                                    target.chmod(0o600)
                        if setup_fault:
                            action, relative = setup_fault
                            target = directory / relative
                            if action == "omit":
                                target.unlink()
                            elif action == "mode":
                                target.chmod(0o777)
                            else:
                                target.write_bytes(b"wrong setup output")
                elif argv[0] == "/bin/bash" or args[0] == "update":
                    name = phase()
                    if name in observer.CLI_REFUSALS:
                        self.returncode = 1
                        if name == "tampered-binary":
                            stdout, stderr = "Downloading binary", "SHA-256 mismatch"
                        elif name == "wrong-platform":
                            stdout, stderr = "Fetching release:", "No release available for platform:" if argv[0] == "/bin/bash" else "No binary CID for platform"
                        elif name == "wrong-version":
                            stdout, stderr = "Downloading binary\nDownloading components", "Downloaded binary version mismatch" if argv[0] == "/bin/bash" else "Installed binary version mismatch"
                        else:
                            stdout = "Fetching release:" if name == "wrong-signer-release" else "Checking for updates"
                            stderr = "Envelope signer differs from the pinned maintainer DID" if argv[0] == "/bin/bash" else "Signer DID mismatch"
                            if argv[0] == "/bin/bash" and name == "wrong-signer-release":
                                stdout = "Verifying release signature"
                    elif argv[0] == "/bin/bash":
                        copy_publication(self.home, name, first=True, gateway=kwargs["env"]["ELASTOS_PUBLISHER_GATEWAY"])
                        lock = self.home / observer.CLI_DATA / "host-process.lock"
                        observer.write(lock, {"pid": self.pid + 100000, "role": "principal-root-upgrade", "addr": "offline"})
                        lock.chmod(0o600)
                    elif args[-1] == "--check":
                        stdout = "Discovery: Carrier"
                    elif apply_exit:
                        self.returncode = apply_exit
                        stdout, stderr = "Discovery: Carrier", apply_stderr
                    else:
                        current = observer.read(self.home / observer.CLI_DATA / "sources.json")["sources"][0]["installed_version"]
                        if current == manifest["new"]["version"]:
                            stdout = "Installed release is up to date."
                        else:
                            copy_publication(self.home, "new")
                            observer.write(self.home / observer.CLI_DATA / "host-process.lock",
                                           {"pid": self.pid + int(coordination_pid_drift), "role": "principal-root-upgrade", "addr": "offline"})
                            if user_data_drift:
                                (self.home / observer.CLI_DATA / "state/value").write_bytes(b"unexpected user data change")
                            stdout, stderr = "Discovery: Carrier", apply_stderr
                    if http_fallback and args[0] == "update":
                        import http.client
                        gateway = observer.read(self.home / observer.CLI_DATA / "sources.json")["sources"][0]["gateways"][0]
                        connection = http.client.HTTPConnection(gateway.removeprefix("http://"), timeout=5)
                        connection.request("GET", "/release.json")
                        connection.getresponse().read()
                        connection.close()
                elif args == ["--version"]:
                    value = (self.home / ".local/bin/elastos").read_bytes()
                    stdout = "elastos " + manifest["old" if value == b"old" else "new"]["version"] + "\n"
                kwargs["stdout"].write(stdout.encode())
                kwargs["stderr"].write(stderr.encode())
                kwargs["stdout"].flush()
                kwargs["stderr"].flush()
            def poll(self):
                return self.returncode
            def wait(self, timeout=None):
                return self.returncode
        def stop(pid, sig):
            proc = next(proc for proc in processes if proc.pid == pid)
            if proc.argv[1] == "gateway" and holder_shutdown_stderr:
                proc.stderr.write(holder_shutdown_stderr.encode())
                proc.stderr.flush()
            proc.returncode = 0
        def census():
            if cleanup_error:
                raise OSError("fixture process census unavailable")
            return [{"pid": proc.pid, "parent": 0, "group": proc.pid, "command": " ".join(proc.argv)} for proc in processes if proc.poll() is None]
        def bootstrap_response(*args, **kwargs):
            nonlocal bootstrap_calls
            bootstrap_calls += 1
            fields = restart_fields if bootstrap_calls > 1 and restart_fields is not None else bootstrap_fields
            # gateway_room.rs deliberately omits DID on its public publisher response.
            return io.BytesIO(json.dumps({"schema": "elastos.carrier.bootstrap/v1", "transport": "carrier",
                                         "role": "publisher", "ticket": public_ticket(), "node_id": HOLDER_NODE,
                                         "generated_at": 1, **(fields or {})}).encode())
        with contextlib.ExitStack() as stack:
            stack.enter_context(patch.object(observer, "cli_admit", return_value=manifest))
            stack.enter_context(patch.object(observer.subprocess, "Popen", Process))
            stack.enter_context(patch.object(observer, "cli_census", side_effect=census))
            stack.enter_context(patch.object(observer.os, "killpg", side_effect=stop))
            stack.enter_context(patch.object(observer.os, "kill", side_effect=stop))
            stack.enter_context(patch.object(observer.time, "sleep"))
            stack.enter_context(patch.object(observer, "health", side_effect=lambda _: any(proc.poll() is None for proc in processes)))
            stack.enter_context(patch.object(observer, "lock_state", side_effect=lambda _: "held" if any(proc.poll() is None for proc in processes) else "released"))
            opener = stack.enter_context(patch.object(observer.urllib.request, "build_opener"))
            opener.return_value.open.side_effect = bootstrap_response
            stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            code = observer.cli_run(self.config, self.root / "results", local_rehearsal)
        return code, calls, observer.read(self.root / "results/result.json")

    def test_local_runner_keeps_full_hop_twelve_refusals_and_owned_cleanup(self):
        reference = self.local_fixture()
        self.manifest["initial_home"] = {"files": []}
        self.freeze()
        with self.local_environment(), patch.object(observer, "cli_initial_home", return_value={"status": "passed"}) as initial_home:
            code, calls, result = self.fake_run(local_rehearsal=reference)
        initial_home.assert_called_once()
        self.assertEqual(code, 0, result)
        self.assertEqual(result["paths"]["m1-install"]["checks"]["initial-home"]["status"], "passed")
        self.assertEqual((result["proof_scope"], result["reference"], result["retrieval_reference"]), ("local-rehearsal", reference, ""))
        self.assertEqual(len([argv for argv, _ in calls if argv[1:] == ["update", "--check"]]), 3)
        for path in result["paths"].values():
            for refusal in observer.CLI_REFUSALS:
                self.assertEqual(path["checks"][refusal]["status"], "passed")
        for refusal in ("wrong-signer-head-check", "wrong-signer-release-check"):
            self.assertEqual(result["paths"]["m2-discovery"]["checks"][refusal]["status"], "passed")
        self.assertTrue(result["cleanup"]["passed"])
        self.assertEqual(result["transport"]["m2_http_fallback_requests"], 0)

    def test_local_runner_keeps_failed_apply_detail_private(self):
        reference = self.local_fixture()
        stderr = "private local fixture error detail\n"
        with self.local_environment():
            code, _, result = self.fake_run(apply_exit=1, apply_stderr=stderr, local_rehearsal=reference)
        self.assertEqual(code, 1)
        self.assertEqual(result["failure"], "plain Carrier apply failed; command exit 1")
        self.assertNotIn("private local", json.dumps(result))
        self.assertEqual((self.root / "results/m2-apply.stderr").read_text(), stderr)
        self.assertEqual((self.root / "results").stat().st_mode & 0o777, 0o700)
        self.assertTrue(result["cleanup"]["passed"])

    def test_local_runner_source_change_fails_receipt_after_owned_cleanup(self):
        reference = self.local_fixture()
        with self.local_environment(), patch.object(observer, "cli_local_rehearsal", side_effect=ValueError("local rehearsal requires a clean admitted source")):
            code, _, result = self.fake_run(local_rehearsal=reference)
        self.assertEqual(code, 1)
        self.assertEqual(result["status"], "failed")
        self.assertIn("local rehearsal requires a clean admitted source", result["cleanup"]["errors"])
        self.assertFalse(result["cleanup"]["passed"])
        self.assertEqual(result["cleanup"]["remaining_pids"], [])

    def test_runner_registers_copied_native_support_before_initial_home_and_cleans_orphans(self):
        for name in ("ipfs-provider", "kubo"):
            target = "bin/" + name
            self.manifest["consumer"]["files"][target] = self.manifest["holder"]["files"][observer.CLI_DATA + "/" + target]
            self.manifest["preserve"]["support"].append(target)
        self.manifest["initial_home"] = {"files": []}
        self.freeze()
        owned_roots = {}
        def initial_home(manager, manifest, home, evidence, publish_next, root):
            for name in ("ipfs-provider", "kubo"):
                target = "bin/" + name
                path = str(home / observer.CLI_DATA / target)
                expected = manifest["files"][manifest["consumer"]["files"][target]]["sha256"]
                self.assertEqual(manager.roots[path], {expected})
                self.assertEqual(observer.digest(Path(path)), expected)
                owned_roots[path] = manager.roots[path]
            return {**evidence, "status": "passed"}
        with patch.object(observer, "cli_initial_home", side_effect=initial_home) as initial:
            code, _, result = self.fake_run()
        self.assertEqual(code, 0, result)
        initial.assert_called_once()
        manager = observer.CliProcesses(self.root)
        manager.roots.update(owned_roots)
        rows = []
        for index, path in enumerate(owned_roots):
            pid = 32000 + index * 2
            rows.extend(({"pid": pid, "parent": 1, "group": pid, "command": path},
                         {"pid": pid + 1, "parent": pid, "group": pid, "command": "/usr/bin/owned-child"}))
        expected_pids = {row["pid"] for row in rows}
        def stop(pid, sig):
            rows[:] = [row for row in rows if row["pid"] != pid]
        with patch.object(observer, "cli_census", side_effect=lambda: list(rows)), \
             patch.object(observer.os, "kill", side_effect=stop) as pids, patch.object(observer.time, "sleep"):
            cleanup = manager.cleanup()
        self.assertEqual({call.args[0] for call in pids.call_args_list}, expected_pids)
        self.assertEqual(cleanup["remaining_pids"], [])
        self.assertFalse(cleanup["errors"])

    def test_actual_runner_orders_fresh_install_plain_check_apply_repeat_and_all_refusals(self):
        code, calls, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertEqual(result["proof_kind"], "harness-self-test")
        self.assertEqual(result["signer_did"], self.manifest["signer_did"])
        self.assertEqual(result["channel"], self.manifest["channel"])
        self.assertTrue(result["cleanup"]["passed"])
        self.assertTrue(all(entry["clean"] for entry in result["holder_output"]))
        self.assertEqual(result["transport"]["m2_http_fallback_requests"], 0)
        coordination = result["coordination"]
        self.assertNotEqual(coordination["m1-install"]["pid"], result["paths"]["m1-install"]["checks"]["positive"]["pid"])
        self.assertNotEqual(coordination["m1-install"]["pid"], coordination["m2-apply"]["pid"])
        self.assertEqual(coordination["m2-apply"]["pid"], result["paths"]["m2-discovery"]["checks"]["apply"]["pid"])
        self.assertEqual(coordination["m2-repeat"], coordination["m2-apply"])
        holder_config = self.root / "results/homes/holder" / observer.CLI_DATA / "config.toml"
        self.assertRegex(holder_config.read_text(), r'^carrier_bind_addr = "127\.0\.0\.1:[1-9][0-9]*"\ngateway_public_publisher_bootstrap = true\n$')
        self.assertEqual(holder_config.stat().st_mode & 0o777, 0o600)
        holder_identity = next(index for index, (argv, _) in enumerate(calls) if argv[1:] == ["node", "info", "--json"])
        first_gateway = next(index for index, (argv, _) in enumerate(calls) if argv[1] == "gateway")
        self.assertLess(holder_identity, first_gateway)
        updates = [argv[1:] for argv, _ in calls if argv[1] == "update"]
        self.assertEqual(updates, [["update", "--check"], ["update"], ["update"], ["update", "--check"], ["update"], ["update", "--check"], ["update"], *([["update"]] * 3)])
        self.assertTrue(all("XDG_DATA_HOME" not in env for _, env in calls))
        self.assertTrue(all("--gateway" not in argv and "source" not in argv for argv, _ in calls))
        for selector in ("m1-install", "m2-discovery"):
            self.assertTrue(set(observer.CLI_REFUSALS) <= set(result["paths"][selector]["checks"]))

    def test_runner_rejects_changed_user_data_despite_valid_coordination(self):
        code, _, result = self.fake_run(user_data_drift=True)
        self.assertEqual(code, 1)
        self.assertIn("config/data/support preservation differs", result["failure"])
        apply = result["paths"]["m2-discovery"]["checks"]["apply"]
        self.assertEqual((apply["status"], apply["exit"]), ("failed", 0))
        self.assertEqual(apply["stderr_sha256"], hashlib.sha256(b"").hexdigest())
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_rejects_substituted_legacy_metadata_before_update(self):
        code, calls, result = self.fake_run(metadata_drift="legacy")
        self.assertEqual(code, 1)
        self.assertIn("legacy installed metadata hash differs", result["failure"])
        self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_rejects_substituted_consumed_metadata_after_apply(self):
        code, _, result = self.fake_run(metadata_drift="consumed")
        self.assertEqual(code, 1)
        self.assertIn("installed consumed metadata hash differs", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_rejects_consumed_head_cid_mismatch(self):
        code, _, result = self.fake_run(metadata_drift="head CID")
        self.assertEqual(code, 1)
        self.assertIn("installed consumed head CID differs", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_rejects_publisher_changes_after_apply(self):
        code, _, result = self.fake_run(metadata_drift="publisher")
        self.assertEqual(code, 1)
        self.assertIn("config/data/support preservation differs", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_runner_rejects_coordination_pid_unbound_to_apply_command(self):
        code, _, result = self.fake_run(coordination_pid_drift=True)
        self.assertEqual(code, 1)
        self.assertIn("coordination PID differs", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_holder_identity_mismatch_stops_before_install_and_cleans_up(self):
        code, calls, result = self.fake_run(bootstrap_fields={"node_id": "f" * 64})
        self.assertEqual(code, 1)
        self.assertFalse(any(argv[0] == "/bin/bash" for argv, _ in calls))
        self.assertIn("transport identity", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_malformed_local_did_stops_before_gateway_and_cleans_up(self):
        code, calls, result = self.fake_run(local_did="did:key:holder")
        self.assertEqual(code, 1)
        self.assertFalse(any(argv[0] == "/bin/bash" or argv[1] == "gateway" for argv, _ in calls))
        self.assertIn("canonical Ed25519", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_release_signer_cannot_be_the_owned_holder_identity(self):
        self.manifest["signer_did"] = HOLDER_DID
        self.freeze()
        code, calls, result = self.fake_run()
        self.assertEqual(code, 1)
        self.assertFalse(any(argv[0] == "/bin/bash" or argv[1] == "gateway" for argv, _ in calls))
        self.assertIn("holder and signer identities coincide", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_holder_node_drift_stops_after_install_before_update_and_cleans_up(self):
        code, calls, result = self.fake_run(restart_fields={"node_id": "f" * 64, "ticket": public_ticket("f" * 64)})
        self.assertEqual(code, 1)
        self.assertTrue(any(argv[0] == "/bin/bash" for argv, _ in calls))
        self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
        self.assertIn("transport identity", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_malformed_holder_ticket_after_restart_stops_before_update_and_cleans_up(self):
        code, calls, result = self.fake_run(restart_fields={"ticket": ""})
        self.assertEqual(code, 1)
        self.assertFalse(any(argv[1] == "update" for argv, _ in calls))
        self.assertIn("ticket encoding", result["failure"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_holder_config_drift_cannot_pass_cleanup(self):
        code, _, result = self.fake_run(config_drift=True)
        self.assertEqual(code, 1)
        self.assertFalse(result["cleanup"]["passed"])
        self.assertIn("holder transport configuration changed", result["cleanup"]["errors"])

    def test_fixture_cannot_override_observer_owned_holder_transport(self):
        relative = self.add("holder-config.toml", b'carrier_bind_addr = "0.0.0.0:4433"\n')
        self.manifest["holder"]["files"][observer.CLI_DATA + "/config.toml"] = relative
        self.freeze()
        code, calls, result = self.fake_run()
        self.assertEqual(code, 1)
        self.assertIn("transport configuration", result["failure"])
        self.assertFalse(any(argv[0] == "/bin/bash" or argv[1] == "gateway" for argv, _ in calls))
        self.assertTrue(result["cleanup"]["passed"])

    def test_zero_exit_with_endpoint_drop_and_cleanup_failure_cannot_pass(self):
        self.positive()
        code, _, result = self.fake_run(apply_stderr="ERROR ungraceful endpoint drop")
        self.assertEqual(code, 1)
        self.assertIn("Carrier apply failed", result["failure"])
        apply = result["paths"]["m2-discovery"]["checks"]["apply"]
        self.assertEqual((apply["status"], apply["exit"]), ("failed", 0))
        self.assertEqual(apply["stderr_sha256"], hashlib.sha256(b"ERROR ungraceful endpoint drop").hexdigest())

    def test_failed_apply_retains_exact_reply_and_bounded_ci_diagnostic(self):
        stderr = "discarded prefix\n" + "x" * 5000 + "\n\x1b[31mError: fixture apply refusal\x1b[0m\n"
        code, calls, result = self.fake_run(apply_exit=1, apply_stderr=stderr)
        self.assertEqual(code, 1)
        checks = result["paths"]["m2-discovery"]["checks"]
        self.assertEqual(set(checks), {"check", "apply"})
        self.assertEqual(checks["check"]["status"], "passed")
        apply = checks["apply"]
        self.assertEqual((apply["status"], apply["exit"]), ("failed", 1))
        owned = next(row for row in result["cleanup"]["owned_processes"] if row["pid"] == apply["pid"])
        self.assertEqual(owned["exit"], 1)
        self.assertGreaterEqual(apply["elapsed_ms"], 0)
        self.assertEqual(apply["stdout_sha256"], hashlib.sha256(b"Discovery: Carrier").hexdigest())
        self.assertEqual(apply["stderr_sha256"], hashlib.sha256(stderr.encode()).hexdigest())
        self.assertTrue(result["failure"].startswith("plain Carrier apply failed; command exit 1; CI m2-apply stderr: "))
        self.assertTrue(result["failure"].endswith("Error: fixture apply refusal"))
        self.assertNotIn("discarded prefix", result["failure"])
        self.assertNotIn("\x1b", result["failure"])
        self.assertLessEqual(len(result["failure"].split("; CI m2-apply stderr: ", 1)[1]), 1024)
        self.assertEqual([argv[1:] for argv, _ in calls if argv[1] == "update"], [["update", "--check"], ["update"]])
        self.assertEqual(observer.digest(self.root / "results/homes/cli/.local/bin/elastos"),
                         self.manifest["files"][self.manifest["publications"]["old"]["binary"]]["sha256"])
        self.assertTrue(result["cleanup"]["passed"])
        self.assertEqual(result["transport"]["m2_http_fallback_requests"], 0)

    def test_failure_keeps_redacted_controller_state_log_tails_and_support_listing(self):
        planted = "planted-attach-secret-0123456789"
        stderr = ("".join("apply line " + str(index) + "\n" for index in range(500))
                  + "attach accepted " + planted + "\n" + "Authorization: Bearer leaked-bearer\n")
        code, _, result = self.fake_run(apply_exit=1, apply_stderr=stderr)
        self.assertEqual(code, 1)
        # The failure text in result.json passes the same redactor.
        self.assertTrue(result["failure"].endswith("Authorization: Bearer [redacted]"))
        output = self.root / "results"
        data = output / "homes/cli" / observer.CLI_DATA
        (data / "update-controller").mkdir(parents=True, exist_ok=True)
        status = data / "update-controller/status.json"
        observer.write(status, {"phase": "restored", "attach_secret": "a" * 64, "home_token": "leaked-token",
                                "detail": {"passkey_credential": "leaked-passkey"}})
        status.chmod(0o600)
        (data / "update-controller/runtime.log").write_text(
            "".join("runtime line " + str(index) + "\n" for index in range(450))
            + 'WARN provider refused {"attach_secret":"leaked-secret"} url=/home?token=leaked-query\n'
            + "child attached with " + planted + "\n")
        # The Home holds this secret in a record; a line carrying it is dropped even unnamed.
        observer.write(data / "gateway-runtime-coords.json", {"attach_secret": planted, "pid": 1})
        (data / "bin").mkdir(exist_ok=True)
        manager = SimpleNamespace(output=output)
        diagnostics = observer.cli_failure_diagnostics(manager, self.manifest, [output / "homes/cli", output / "homes/absent"], "probe")
        self.assertEqual(diagnostics["errors"], [])
        written = {name: (output / name).read_text() for name in diagnostics["files"]}
        prefix = "diagnostics/probe/"
        self.assertEqual(observer.cli_json(output / (prefix + "cli/update-controller-status.json")), {"phase": "restored"})
        runtime = written[prefix + "cli/logs/update-controller__runtime.log"].splitlines()
        self.assertEqual((len(runtime), runtime[0]), (399, "runtime line 52"))
        self.assertIn("WARN provider refused", runtime[-1])
        apply = written[prefix + "output/m2-apply.stderr"].splitlines()
        self.assertEqual((len(apply), apply[-1]), (399, "Authorization: Bearer [redacted]"))
        detail = observer.cli_ci_stderr_detail(manager, self.manifest, "m2-apply", "m2-apply")
        self.assertTrue(detail.endswith("Authorization: Bearer [redacted]") and "apply line 499" in detail)
        self.assertNotIn(planted, detail)
        listing = written[prefix + "cli/ls-bin-installation.txt"]
        self.assertIn("$ ls -la bin\n", listing)
        self.assertIn("$ ls -la installation\n", listing)
        self.assertFalse(any("leaked" in text or "a" * 64 in text or planted in text for text in written.values()))
        # The run's own failure path collected the same kinds of files before cleanup.
        self.assertIn("diagnostics/after-failure/output/m2-apply.stderr", result["diagnostics"]["files"])
        self.assertIn("diagnostics/after-failure/cli/ls-bin-installation.txt", result["diagnostics"]["files"])
        # Production fixtures keep operator logs private.
        self.assertEqual(observer.cli_failure_diagnostics(manager, {**self.manifest, "proof_scope": "production-positive"},
                                                         [output / "homes/cli"], "private"), {})

    def test_failed_operator_apply_keeps_diagnostic_private_and_reply_available(self):
        self.positive()
        stderr = "private operator apply diagnostic\n"
        code, _, result = self.fake_run(apply_exit=1, apply_stderr=stderr)
        self.assertEqual(code, 1)
        self.assertEqual(result["failure"], "plain Carrier apply failed; command exit 1")
        self.assertNotIn("private operator", json.dumps(result))
        apply = result["paths"]["m2-discovery"]["checks"]["apply"]
        self.assertEqual((apply["status"], apply["exit"]), ("failed", 1))
        self.assertEqual(apply["stderr_sha256"], hashlib.sha256(stderr.encode()).hexdigest())
        self.assertTrue(result["cleanup"]["passed"])

    def test_apply_reply_survives_coordination_inspection_refusal(self):
        inspect = observer.cli_coordination
        def coordination(home_path, required=True):
            if home_path.name == "cli" and (self.root / "results/m2-apply.stderr").exists():
                raise ValueError("injected apply coordination refusal")
            return inspect(home_path, required)
        with patch.object(observer, "cli_coordination", side_effect=coordination):
            code, _, result = self.fake_run()
        self.assertEqual(code, 1)
        self.assertEqual(result["failure"], "injected apply coordination refusal")
        checks = result["paths"]["m2-discovery"]["checks"]
        self.assertEqual(checks["check"]["status"], "passed")
        self.assertEqual((checks["apply"]["status"], checks["apply"]["exit"]), ("failed", 0))
        self.assertEqual(checks["apply"]["stdout_sha256"], hashlib.sha256(b"Discovery: Carrier").hexdigest())
        self.assertNotIn("m2-apply", result["coordination"])
        self.assertTrue(result["cleanup"]["passed"])

    def test_cleanup_census_failure_keeps_result_failed(self):
        code, _, result = self.fake_run(cleanup_error=True)
        self.assertEqual(code, 1)
        self.assertFalse(result["cleanup"]["passed"])

    def test_holder_endpoint_error_after_success_cannot_pass(self):
        code, _, result = self.fake_run(holder_stderr="ERROR ungraceful endpoint drop")
        self.assertEqual(code, 1)
        self.assertTrue(all(path["status"] == "passed" for path in result["paths"].values()))
        self.assertFalse(result["cleanup"]["passed"])
        self.assertTrue(result["transport"]["installer_bootstrap_closed"])
        self.assertIn("holder output", result["cleanup"]["errors"][0])

    def test_holder_shutdown_error_cannot_pass(self):
        code, _, result = self.fake_run(holder_shutdown_stderr="ERROR ungraceful endpoint drop")
        self.assertEqual(code, 1)
        self.assertFalse(result["cleanup"]["passed"])

    def test_correct_reply_via_http_fallback_cannot_pass(self):
        code, _, result = self.fake_run(http_fallback=True)
        self.assertEqual(code, 1)
        self.assertGreater(result["transport"]["m2_http_fallback_requests"], 0)
        self.assertIn("HTTP fallback", result["cleanup"]["errors"][0])

    def test_bootstrap_serves_only_admitted_m1_bytes_then_refuses_m2(self):
        import http.client
        server = observer.CliBootstrap(self.root, self.manifest)
        try:
            server.publication = self.manifest["publications"]["old"]
            server.enabled = True
            connection = http.client.HTTPConnection(server.url.removeprefix("http://"), timeout=5)
            connection.request("GET", "/release.json")
            response = connection.getresponse()
            self.assertEqual(response.status, 200)
            self.assertEqual(response.read(), (self.root / server.publication["release"]).read_bytes())
            connection.close()
            server.enabled = False
            connection = http.client.HTTPConnection(server.url.removeprefix("http://"), timeout=5)
            connection.request("GET", "/release.json")
            response = connection.getresponse()
            self.assertEqual(response.status, 503)
            response.read()
            connection.close()
            self.assertEqual(server.fallback_requests, 1)
        finally:
            server.close()


if __name__ == "__main__":
    unittest.main()
