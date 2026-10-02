#!/usr/bin/env python3
"""Observer regressions with mocked processes and transport; no Runtime Homes."""

import contextlib
import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
import subprocess
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
                         "proof_scope": "ci-refusals", "proof_kind": "harness-self-test", "source": {"commit": "a" * 40, "tree": "b" * 40},
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
                       "version": self.manifest["old" if phase == "old" else "new"]["version"],
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
        self.manifest["preserve"] = {"config": ["config"], "data": ["state"], "support": ["capsules"]}
        self.config = {"schema": self.manifest["schema"], "mode": observer.CLI_MODE, "root": str(self.root),
                       "immutable": {"reference": self.manifest["reference"], "manifest": "manifest.json"}}
        self.freeze()
        self.env = patch.dict(observer.os.environ, {"ELASTOS_CI_FIXTURE_MANIFEST_SHA256": self.config["immutable"]["sha256"],
                                                    "ELASTOS_CI_FIXTURE_REFERENCE": self.manifest["reference"], "ELASTOS_CI_REQUIRE_REAL_RUNTIME": "0", "ELASTOS_CI_FIXTURE_SCOPE": "ci-refusals"})
        self.env.start()
        self.addCleanup(self.env.stop)

    def freeze(self):
        observer.write(self.root / "manifest.json", self.manifest)
        self.config["immutable"]["sha256"] = observer.digest(self.root / "manifest.json")
        if hasattr(self, "env"):
            observer.os.environ["ELASTOS_CI_FIXTURE_MANIFEST_SHA256"] = self.config["immutable"]["sha256"]

    def admit(self):
        with patch.object(observer, "cli_installer_metadata", return_value=[self.manifest["signer_did"], ""]), patch.object(observer, "cli_signature") as signatures:
            admitted = observer.cli_admit(self.config)
            self.assertEqual(signatures.call_count, 3 * len(self.manifest["publications"]))
            return admitted

    def positive(self):
        self.manifest["proof_scope"] = "production-positive"
        self.manifest["publications"] = {"old": self.manifest["publications"]["old"],
                                          "new": self.manifest["publications"]["wrong-signer-head"].copy()}
        pub = self.manifest["publications"]["new"]
        head = observer.cli_json(self.root / pub["head"])
        head["signer_did"] = self.manifest["signer_did"]
        pub["head"] = self.add("positive/head.json", head)
        pub["receipt"] = self.add("positive/receipt.json", {"last_head_cid": self.manifest["files"][pub["head"]]["cid"],
                                                            "last_release_cid": self.manifest["files"][pub["release"]]["cid"]}, cid=False)
        self.manifest["holder"]["content"][self.manifest["files"][pub["head"]]["cid"]] = pub["head"]
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
                         [["update", "--check"], ["update", "--yes"], ["update", "--yes"]])
        self.assertEqual(set(result["paths"]["m2-discovery"]["checks"]), {"check", "apply", "repeat"})

    def test_disposable_package_cannot_enter_real_positive_admission(self):
        observer.os.environ["ELASTOS_CI_FIXTURE_SCOPE"] = "production-positive"
        with self.assertRaisesRegex(ValueError, "proof scope differs"):
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

    def test_generator_uses_explicit_disposable_keys_and_real_signature_admission(self):
        # Fake command delivery exercises generation; existing RFC 8032 test
        # signing supplies valid signatures to the actual installer verifier.
        # This proves observer admission only, and reports harness-self-test.
        native = self.root / "native"
        native.write_bytes(b"\xcf\xfa\xed\xfe" + (0x0100000c).to_bytes(4, "little") + b"\0" * 4 + (2).to_bytes(4, "little") + b"\0" * 16)
        support = self.root / "support"
        (support / "bin").mkdir(parents=True)
        for name in ("ipfs-provider", "kubo"):
            (support / "bin" / name).write_bytes(name.encode())
        observer.write(support / "components.json", {"external": {name: {"platforms": {"darwin-arm64": {
            "checksum": "sha256:" + observer.digest(support / "bin" / name)}}} for name in ("ipfs-provider", "kubo")}})
        generated = self.root / "generated"
        real_run, key_paths = observer.subprocess.run, []
        def command(argv, **kwargs):
            if Path(argv[0]).name == "elastos":
                if argv[1] == "--version":
                    return subprocess.CompletedProcess(argv, 0, b"elastos 0.7.1\n", b"")
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
                cid = "b" + base64.b32encode(b"\x01\x55\x12\x20" + hashlib.sha256(raw).digest()).decode().lower().rstrip("=")
                (repo / "blocks" / cid).write_bytes(raw)
                return subprocess.CompletedProcess(argv, 0, (cid + "\n").encode(), b"")
            return real_run(argv, **kwargs)
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
             patch.object(observer.subprocess, "run", side_effect=command), patch.object(observer.subprocess, "check_output", return_value="a" * 40 + "\n"):
            receipt = observer.cli_generate_refusals(generated, native, support)
        self.assertTrue(receipt["keys_removed"])
        self.assertTrue(key_paths)
        self.assertTrue(all(not path.exists() for path in key_paths))
        self.assertFalse((generated / "generator").exists())
        generated_manifest = observer.cli_json(generated / "manifest.json")
        self.assertEqual(set(generated_manifest["publications"]), set(observer.CLI_PHASES))
        self.assertNotEqual(generated_manifest["signer_did"], observer.cli_json(generated / generated_manifest["publications"]["wrong-signer-head"]["head"])["signer_did"])
        generated_manifest["proof_kind"] = "harness-self-test"
        observer.write(generated / "manifest.json", generated_manifest)
        config = observer.cli_json(generated / "fixture.json")
        config["immutable"]["sha256"] = observer.digest(generated / "manifest.json")
        with patch.dict(observer.os.environ, {"ELASTOS_CI_FIXTURE_MANIFEST_SHA256": config["immutable"]["sha256"], "ELASTOS_CI_FIXTURE_REFERENCE": ""}):
            self.assertEqual(observer.cli_admit(config)["proof_kind"], "harness-self-test")

    def test_generator_refuses_operator_or_non_ci_signing(self):
        with patch.dict(observer.os.environ, {"CI": "false", "GITHUB_ACTIONS": "false"}), self.assertRaisesRegex(ValueError, "native Mac CI"):
            observer.cli_generate_refusals(self.root / "generated", self.root / "missing", self.root)

    def test_generator_command_failure_removes_keys_and_repository(self):
        support = self.root / "support"
        (support / "bin").mkdir(parents=True)
        for name in ("ipfs-provider", "kubo"):
            (support / "bin" / name).write_bytes(name.encode())
        runtime = self.root / "runtime"
        runtime.write_bytes(b"Runtime command substitute")
        generated = self.root / "generated"
        def failed(argv, **kwargs):
            self.assertTrue((generated / "generator/approved.key").is_file())
            self.assertTrue((generated / "generator/other.key").is_file())
            return subprocess.CompletedProcess(argv, 1, b"", b"private command error stays out of receipts")
        with patch.dict(observer.os.environ, {"CI": "true", "GITHUB_ACTIONS": "true", "GITHUB_RUN_ID": "123", "GITHUB_RUN_ATTEMPT": "1"}), \
             patch.object(observer.sys, "platform", "darwin"), patch.object(observer.platform, "machine", return_value="arm64"), \
             patch.object(observer.subprocess, "run", side_effect=failed), patch.object(observer.subprocess, "check_output", return_value="a" * 40 + "\n"), \
             self.assertRaisesRegex(ValueError, "disposable fixture command failed"):
            observer.cli_generate_refusals(generated, runtime, support)
        self.assertFalse((generated / "generator").exists())

    def fake_run(self, apply_stderr="", local_did=HOLDER_DID, bootstrap_fields=None, restart_fields=None,
                 cleanup_error=False, holder_stderr="", holder_shutdown_stderr="", http_fallback=False,
                 config_drift=False):
        manifest, root, calls, processes = self.manifest, self.root, [], []
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
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_bytes((root / pub[key]).read_bytes())
            if first:
                source = {"name": "default", "publisher_dids": [manifest["signer_did"]], "channel": "canary", "connect_ticket": public_ticket(),
                          "publisher_node_id": HOLDER_NODE, "install_path": str(home_path / ".local/bin/elastos"), "installed_version": manifest[name]["version"], "head_cid": "", "gateways": [gateway]}
                observer.write(directory / "sources.json", {"default_source": "default", "sources": [source]})
            else:
                sources = observer.read(directory / "sources.json")
                sources["sources"][0].update(installed_version=manifest[name]["version"], head_cid=manifest["files"][pub["head"]]["cid"])
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
                    elif args[-1] == "--check":
                        stdout = "Discovery: Carrier"
                    else:
                        current = observer.read(self.home / observer.CLI_DATA / "sources.json")["sources"][0]["installed_version"]
                        if current == manifest["new"]["version"]:
                            stdout = "Installed release is up to date."
                        else:
                            copy_publication(self.home, "new")
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
            code = observer.cli_run(self.config, self.root / "results")
        return code, calls, observer.read(self.root / "results/result.json")

    def test_actual_runner_orders_fresh_install_plain_check_apply_repeat_and_all_refusals(self):
        code, calls, result = self.fake_run()
        self.assertEqual(code, 0, result)
        self.assertEqual(result["proof_kind"], "harness-self-test")
        self.assertEqual(result["signer_did"], self.manifest["signer_did"])
        self.assertEqual(result["channel"], self.manifest["channel"])
        self.assertTrue(result["cleanup"]["passed"])
        self.assertTrue(all(entry["clean"] for entry in result["holder_output"]))
        self.assertEqual(result["transport"]["m2_http_fallback_requests"], 0)
        holder_config = self.root / "results/homes/holder" / observer.CLI_DATA / "config.toml"
        self.assertRegex(holder_config.read_text(), r'^carrier_bind_addr = "127\.0\.0\.1:[1-9][0-9]*"\ngateway_public_publisher_bootstrap = true\n$')
        self.assertEqual(holder_config.stat().st_mode & 0o777, 0o600)
        holder_identity = next(index for index, (argv, _) in enumerate(calls) if argv[1:] == ["node", "info", "--json"])
        first_gateway = next(index for index, (argv, _) in enumerate(calls) if argv[1] == "gateway")
        self.assertLess(holder_identity, first_gateway)
        updates = [argv[1:] for argv, _ in calls if argv[1] == "update"]
        self.assertEqual(updates, [["update", "--check"], ["update", "--yes"], ["update", "--check"], ["update", "--yes"], *([["update", "--yes"]] * 3)])
        self.assertTrue(all("XDG_DATA_HOME" not in env for _, env in calls))
        self.assertTrue(all("--gateway" not in argv and "source" not in argv for argv, _ in calls))
        for selector in ("m1-install", "m2-discovery"):
            self.assertTrue(set(observer.CLI_REFUSALS) <= set(result["paths"][selector]["checks"]))

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
