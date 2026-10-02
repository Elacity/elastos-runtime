#!/usr/bin/env python3
"""Observer regressions with mocked processes and transport; no Runtime Homes."""

import contextlib
import importlib.util
import io
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True
spec = importlib.util.spec_from_file_location("observer", Path(__file__).with_name("update-hop-compare.py"))
observer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(observer)


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


if __name__ == "__main__":
    unittest.main()
