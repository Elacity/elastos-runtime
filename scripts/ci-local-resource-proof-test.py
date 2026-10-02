#!/usr/bin/env python3
"""Light refusal checks for the installed Linux resource proof harness."""
import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock

SPEC = importlib.util.spec_from_file_location("resource_proof", Path(__file__).with_name("ci-local-resource-proof.py"))
PROOF = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PROOF)
ROOT = Path("/sys/fs/cgroup")
LEAF = ROOT / "system.slice/elastos-la04-lifecycle-test.service"
MEMBERSHIP = "0::/system.slice/elastos-la04-lifecycle-test.service\n"
MOUNTS = "1 0 0:1 / /sys/fs/cgroup rw - cgroup2 cgroup rw\n"


def cgroup_files(limit=PROOF.GIB):
    values = {}
    for directory, maximum in ((LEAF, str(limit)), (ROOT / "system.slice", "max")):
        values[directory / "cgroup.controllers"] = ""
        values[directory / "memory.max"] = maximum
        values[directory / "memory.current"] = "100"
    values[ROOT / "cgroup.controllers"] = "cpu memory pids"
    values[LEAF / "memory.swap.max"] = "0"
    values[LEAF / "pids.max"] = "512"
    return values


def read_values(values):
    def read(path):
        if path not in values:
            raise FileNotFoundError()
        if isinstance(values[path], Exception):
            raise values[path]
        return values[path]
    return read


def outcome(qualification):
    row = dict(qualification=qualification, status="passed", final_descendants=0, account_lease_released=True)
    if qualification == "low-memory":
        row.update(error_class="context_rejected", error_code="model_memory_unavailable",
                   actionable_message=True, guard_start_observed=False)
    else:
        row.update(reply=True, shared_busy=True, idle_release=True, crash_cleanup=True, recovery=True)
    return row


class CgroupTests(unittest.TestCase):
    def test_full_global_ancestry_and_exact_two_limits(self):
        for limit in (PROOF.GIB, 4 * PROOF.GIB):
            observation, leaf = PROOF.cgroup_observation(limit, MEMBERSHIP, MOUNTS, read=read_values(cgroup_files(limit)))
            self.assertEqual(leaf, LEAF)
            self.assertEqual(observation["effective_limit_bytes"], limit)
            self.assertEqual([row["memory_max_bytes"] for row in observation["ancestors"]], [limit, None])
            self.assertNotIn("/", json.dumps(observation))

    def test_cgroup_refusals_precede_test_process(self):
        cases = []
        values = cgroup_files(); del values[LEAF / "memory.current"]
        cases.append((values, MEMBERSHIP, MOUNTS))
        values = cgroup_files(); values[ROOT / "cgroup.controllers"] = PermissionError()
        cases.append((values, MEMBERSHIP, MOUNTS))
        values = cgroup_files(); values[ROOT / "memory.max"] = "max"
        cases.append((values, MEMBERSHIP, MOUNTS))
        cases.append((cgroup_files(), MEMBERSHIP + MEMBERSHIP, MOUNTS))
        values = cgroup_files(4 * PROOF.GIB)
        cases.append((values, MEMBERSHIP, MOUNTS))
        values = cgroup_files(); values[ROOT / "system.slice/memory.max"] = str(PROOF.GIB // 2)
        cases.append((values, MEMBERSHIP, MOUNTS))
        values = cgroup_files(); values[ROOT / "cgroup.controllers"] = "cpu pids"
        cases.append((values, MEMBERSHIP, MOUNTS))
        cases.append((cgroup_files(), MEMBERSHIP, MOUNTS.replace(" / /sys", " /hidden /sys")))
        cases.append((cgroup_files(), MEMBERSHIP, MOUNTS + MOUNTS))
        cases.append((cgroup_files(), MEMBERSHIP + "1:memory:/legacy\n", MOUNTS))
        for filename, value in (("memory.swap.max", "max"), ("pids.max", "511"), ("memory.max", "+1073741824")):
            values = cgroup_files(); values[LEAF / filename] = value
            cases.append((values, MEMBERSHIP, MOUNTS))
        observe = PROOF.cgroup_observation
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            (fixture / "plan.private.json").write_text(json.dumps({"uid": os.getuid(), "gid": os.getgid()}))
            for values, membership, mounts in cases:
                with self.subTest(membership=membership, values=values), mock.patch.object(PROOF, "cgroup_observation",
                        side_effect=lambda expected: observe(expected, membership, mounts, read=read_values(values))), \
                        mock.patch.object(PROOF.subprocess, "run") as process:
                    with self.assertRaises((RuntimeError, OSError)):
                        PROOF.unit(fixture, fixture / "test", "low-memory", PROOF.GIB)
                    process.assert_not_called()


class IdentityTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.data = self.root / "data"
        (self.data / "bin").mkdir(parents=True)
        (self.data / "receipts").mkdir()
        self.source = dict(commit="a" * 40, tree="b" * 40, clean=True)
        self.host = "linux-amd64"
        for name in ("elastos", "model-provider"):
            (self.data / "bin" / name).write_bytes(name.encode())
            (self.root / name).write_bytes(name.encode())
        self.receipt = dict(schema="elastos.source-home.installation-receipt/v1", source=self.source,
                runtime=dict(parity=True, built_sha256=PROOF.file_hash(self.root / "elastos"),
                             installed_sha256=PROOF.file_hash(self.root / "elastos")), platform=self.host)
        self.manifest = {"external": {"model-provider": {"platforms": {self.host:
                {"install_path": "bin/model-provider", "checksum": PROOF.file_hash(self.root / "model-provider")}}}}}
        self.save()

    def save(self):
        (self.data / "receipts/source-home-installation.json").write_text(json.dumps(self.receipt))
        (self.data / "components.json").write_text(json.dumps(self.manifest))

    def verify(self):
        with mock.patch.object(PROOF.platform, "machine", return_value="x86_64"):
            return PROOF.installed_identity(self.root, self.data, self.source, self.root / "model-provider", self.root / "elastos")

    def test_matching_installed_built_and_manifest_bytes(self):
        _, _, row = self.verify()
        self.assertEqual(row["source_tree"], self.source["tree"])
        self.assertEqual(row["provider_sha256"], PROOF.file_hash(self.root / "model-provider"))
        self.assertNotIn(str(self.root), json.dumps(row))

    def test_source_and_artifact_mismatches_refused(self):
        actions = [lambda: self.receipt.update(source=dict(self.source, tree="c" * 40)),
                   lambda: self.receipt["runtime"].update(parity=False),
                   lambda: self.manifest["external"]["model-provider"]["platforms"][self.host].update(checksum="sha256:" + "0" * 64),
                   lambda: (self.root / "model-provider").write_bytes(b"stale"),
                   lambda: (self.data / "bin/elastos").write_bytes(b"stale")]
        for action in actions:
            with self.subTest(action=action):
                self.setUp()
                action(); self.save()
                with mock.patch.object(PROOF.subprocess, "run") as process, self.assertRaises(RuntimeError):
                    self.verify()
                process.assert_not_called()

    def test_duplicate_receipt_keys_refused(self):
        path = self.data / "receipts/source-home-installation.json"
        path.write_text('{"schema":"old","schema":"new"}')
        with self.assertRaisesRegex(RuntimeError, "duplicate_json_key"):
            self.verify()

    def test_full_engine_bundle_checks_extra_changed_and_aliased_files(self):
        bundle = self.root / "engine"
        bundle.mkdir()
        binary = bundle / "llama-server"
        binary.write_bytes(b"engine"); binary.chmod(0o500)
        library = bundle / "libengine.so"
        library.write_bytes(b"library"); library.chmod(0o400)
        receipt = dict(schema="elastos.local-model-engine/v2", platform=self.host, version="fixture",
                archive_sha256="sha256:" + "d" * 64, entries=[
                    dict(path="libengine.so", type="file", sha256=PROOF.file_hash(library)),
                    dict(path="llama-server", type="file", sha256=PROOF.file_hash(binary))])
        receipt_path = bundle / ".elastos-engine.json"
        receipt_path.write_text(json.dumps(receipt)); receipt_path.chmod(0o400)
        bundle.chmod(0o500)
        component = dict(version="fixture", platforms={self.host:
                dict(checksum=receipt["archive_sha256"], binary_path="llama-server")})
        identity = PROOF.bundle_identity(bundle, component, self.host)
        self.assertEqual(identity["engine_sha256"], PROOF.file_hash(binary))
        library.chmod(0o600); library.write_bytes(b"changed"); library.chmod(0o400)
        with self.assertRaisesRegex(RuntimeError, "engine_inventory"):
            PROOF.bundle_identity(bundle, component, self.host)
        library.chmod(0o600); library.write_bytes(b"library"); library.chmod(0o400)
        bundle.chmod(0o700)
        (bundle / "extra").write_bytes(b"extra"); (bundle / "extra").chmod(0o400)
        bundle.chmod(0o500)
        with self.assertRaisesRegex(RuntimeError, "engine_inventory"):
            PROOF.bundle_identity(bundle, component, self.host)
        bundle.chmod(0o700); (bundle / "extra").unlink()
        library.unlink(); library.symlink_to(self.root / "elastos")
        bundle.chmod(0o500)
        with self.assertRaisesRegex(RuntimeError, "unsafe_relative_path"):
            PROOF.bundle_identity(bundle, component, self.host)
        bundle.chmod(0o700)  # Permit TemporaryDirectory to remove the refused fixture.

    def test_unit_account_mismatch_precedes_process(self):
        (self.root / "plan.private.json").write_text(json.dumps({"uid": os.getuid() + 1, "gid": os.getgid()}))
        with mock.patch.object(PROOF.subprocess, "run") as process, self.assertRaisesRegex(RuntimeError, "unit_account"):
            PROOF.unit(self.root, self.root / "test", "low-memory", PROOF.GIB)
        process.assert_not_called()

    def test_stale_input_directory_precedes_verifier_and_compiler(self):
        (self.root / "resource-proof-local-1").mkdir()
        manifest = {"external": {"llama-server": {"platforms": {self.host: {"install_path": "engine"}}}}}
        with mock.patch.object(PROOF.sys, "platform", "linux"), mock.patch.dict(os.environ, {}, clear=True), \
                mock.patch.dict(PROOF.HOME_HELPERS, disk_observation=lambda home: {"available_bytes": 1000, "capacity_bytes": 1000}), \
                mock.patch.object(PROOF, "source_identity", return_value=self.source), \
                mock.patch.object(PROOF, "installed_identity", return_value=(manifest, self.host, {})), \
                mock.patch.object(PROOF, "bundle_identity", return_value={}), \
                mock.patch.object(PROOF, "file_hash", return_value="sha256:" + PROOF.MODEL_SHA), \
                mock.patch.object(PROOF.subprocess, "run") as process:
            with self.assertRaisesRegex(RuntimeError, "stale_fixture"):
                PROOF.run(self.root, self.root, self.data, self.root)
            process.assert_not_called()
        receipt = json.loads((self.root / "la04-result.json").read_text())
        self.assertEqual(receipt["status"], "failed")
        self.assertEqual(receipt["stage"], "preflight")
        self.assertEqual(receipt["qualifications"], {})
        self.assertEqual(set(receipt["results"].values()), {"failed or not run"})
        self.assertNotIn(str(self.root), json.dumps(receipt))


class ResultTests(unittest.TestCase):
    def test_outcome_requires_every_asserted_result_and_exact_types(self):
        for qualification in ("low-memory", "lifecycle"):
            expected = outcome(qualification)
            self.assertEqual(PROOF.validate_outcome(expected, qualification), expected)
            for key in expected:
                row = dict(expected); del row[key]
                with self.subTest(qualification=qualification, missing=key), self.assertRaises(RuntimeError):
                    PROOF.validate_outcome(row, qualification)
            for key in ("final_descendants", "account_lease_released"):
                row = dict(expected); row[key] = False if key == "final_descendants" else 1
                with self.assertRaises(RuntimeError):
                    PROOF.validate_outcome(row, qualification)

    def test_retained_unit_orphans_are_refused_before_cleanup(self):
        with tempfile.TemporaryDirectory() as directory:
            fixture = Path(directory)
            row = dict(checked_before_unit_exit=True, final_unit_descendants=0,
                       cgroup=dict(effective_limit_bytes=PROOF.GIB), outcome=outcome("low-memory"))
            (fixture / "unit-result.private.json").write_text(json.dumps(row))
            state = dict(ActiveState="active", SubState="exited", Result="success", ExecMainStatus="0",
                         MainPID="0", ControlGroup="/system.slice/elastos-la04-low-memory-test.service")
            with mock.patch.object(PROOF, "unit_state", return_value=state), \
                    mock.patch.object(PROOF, "unit_processes", return_value={123}), \
                    mock.patch.object(PROOF.subprocess, "run") as process:
                with self.assertRaisesRegex(RuntimeError, "unit_descendants_remain"):
                    PROOF.wait_for_unit("owned", fixture, PROOF.GIB)
                process.assert_not_called()
            with mock.patch.object(PROOF, "unit_state", return_value=state), \
                    mock.patch.object(PROOF, "unit_processes", return_value=set()):
                self.assertEqual(PROOF.wait_for_unit("owned", fixture, PROOF.GIB)["retained_unit_descendants"], 0)


if __name__ == "__main__":
    unittest.main()
