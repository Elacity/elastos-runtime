#!/usr/bin/env python3
"""Tiny V4 assembly fixtures: no builds, keys, providers, downloads or real artifacts."""

import copy
import importlib.util
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

sys.dont_write_bytecode = True


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


ROOT = Path(__file__).resolve().parents[2]
F = load("v4_finalizer_fixture", Path(__file__).with_name("canary-v4-finalize.py"))
N = load("v4_native_fixture", ROOT / "scripts/release-platform-input.py")
S = load("v4_signer_fixture", ROOT / "scripts/release-signer.py")


class NativeFixture:
    """Native origin admission is injected; production assembly/CID/signing APIs run."""
    file_record = staticmethod(N.file_record)
    digest = staticmethod(N.digest)

    def __init__(self, test):
        self.test = test

    def verify(self, root):
        receipt = json.loads((root / "platform-input.json").read_bytes())
        for name, pin in receipt["files"].items():
            F.require(N.file_record(root / name) == pin, "native fixture artifact mutation")
        return receipt

    def public_catalog_verification(self, data, did, openssl, scratch):
        envelope = S.parse_json(data)
        F.require(envelope["signer_did"] == did, "catalogue signer differs")
        return envelope, {}, {"path": str(openssl), "sha256": N.digest(openssl)}

    def installer_source_blob(self, source):
        return "a" * 40, self.test.template

    def contexts(self):
        return (patch.object(N, "source_identity", return_value=self.test.source),
                patch.object(N.integrity, "audit_manifest", return_value=[]),
                patch.object(N.integrity, "audit_release_artifacts", return_value=[]))

    def stage_inputs(self, values, version, output, platform):
        receipt = self.verify(self.test.args.finalized_n3)
        values = values or [platform + "=" + str(self.test.args.finalized_n3)]
        with patch.object(N, "validate_inputs", return_value={platform: receipt}), \
                patch.object(N, "merged_input_components", return_value=self.test.components), \
                self.contexts()[0], self.contexts()[1], self.contexts()[2]:
            return N.stage_inputs(values, version, output, platform)

    def verify_staged_inputs(self, stage, preview_platform=None):
        with self.contexts()[0], self.contexts()[1], self.contexts()[2]:
            return N.verify_staged_inputs(stage, preview_platform=preview_platform)

    def attach_input_cids(self, stage, cids, platform):
        with self.contexts()[0], self.contexts()[1], self.contexts()[2]:
            return N.attach_input_cids(stage, cids, platform)

    def signing_input(self, *args):
        with self.contexts()[0], self.contexts()[1], self.contexts()[2], \
                patch.object(N, "installer_source_blob", side_effect=self.installer_source_blob):
            return N.signing_input(*args)


class FinalizationTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(dir=Path.home())
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        guard = patch.object(F, "check_checkouts", return_value=ROOT)
        guard.start()
        self.addCleanup(guard.stop)
        self.source = {"commit": F.COMMIT, "tree": F.TREE, "clean": True, "lockfiles": {"Cargo.lock": "a" * 64}}
        for name in ("N3", "N4", "N3-finalized", "unsigned-V3"):
            (self.root / name).mkdir(mode=0o700)
        self.args = SimpleNamespace(n3=self.root / "N3", n4=self.root / "N4",
            finalized_n3=self.root / "N3-finalized", unsigned_v3=self.root / "unsigned-V3",
            publish_state=self.root / "publish-state.json", runtime_import=self.root / "runtime-import.json",
            openssl=self.root / "openssl", output=self.root / "V4-finalized", workflow_commit="b" * 40,
            source_root=self.root)
        self.put(self.args.openssl, b"inert public-verification tool fixture")
        binary = bytearray(64)
        binary[:4] = bytes.fromhex("cffaedfe")
        struct.pack_into("<I", binary, 4, 0x100000C)
        struct.pack_into("<I", binary, 12, 2)
        n3_binary, n4_binary = bytes(binary) + b"V3", bytes(binary) + b"V4"
        self.catalogue = S.json_bytes({"payload": {"schema": "elastos.model.catalog/v1"},
            "signature": "0" * 128, "signer_did": F.DID})
        self.components = {"schema": "elastos.components/v1", "external": {}, "capsules": {},
            "model_catalog": {"head_cid": S.raw_cid(self.catalogue), "publisher_dids": [F.DID]},
            "model_retention": {str(i): {kind: {"release_path": f"model-{i}.{kind}"}
                for kind in ("car", "receipt")} for i in range(4)}}
        for root, payload, version in ((self.args.n3, n3_binary, "0.8.0-alpha.3"),
                (self.args.n4, n4_binary, "0.8.0-alpha.4"), (self.args.finalized_n3, n3_binary, "0.8.0-alpha.3")):
            (root / "artifacts").mkdir(mode=0o700)
            self.put(root / "artifacts" / F.RUNTIME, payload, 0o700)
            self.put(root / "artifacts/model-catalog.json", self.catalogue)
            for i in range(4):
                for kind in ("car", "receipt"):
                    self.put(root / "artifacts" / f"model-{i}.{kind}", f"tiny {i} {kind}".encode())
            self.put(root / "components.json", S.json_bytes(self.components))
            self.receipt(root, version)
        original = (self.args.n3 / "platform-input.json").read_bytes()
        self.put(self.args.n4 / "support-input.json", original)
        self.receipt(self.args.n4, "0.8.0-alpha.4", support_origin={"receipt_path": "support-input.json"})
        self.put(self.args.finalized_n3 / "model-native-input.json", original)
        self.receipt(self.args.finalized_n3, "0.8.0-alpha.3", model_finalization={
            "publisher_did": F.DID, "native_receipt_sha256": S.sha256(original)})
        (self.root / "receipts").mkdir(mode=0o700)
        self.put(self.root / "receipts/native-pair.json", S.json_bytes({
            "schema": "elastos.canary-union-native-pair/v1",
            "source_commit": F.COMMIT, "source_tree": F.TREE, "source_ci_run": "37238542483",
            "source_clean_before_and_after": True, "support_origin_matches_V3": True, "models_exported": True,
            "publisher_did": F.DID, "workflow_commit": "c" * 40, "run": "37254309574", "attempt": "1",
            "native": [{"input": name, "version": version, "stdout": "elastos " + version + "\n", "stderr": "",
                "binary_sha256": N.digest(root / "artifacts" / F.RUNTIME)}
                for name, root, version in (("N3", self.args.n3, "0.8.0-alpha.3"),
                                           ("N4", self.args.n4, "0.8.0-alpha.4"))]}))
        self.template = (" ".join("__" + key + "__" for key in sorted(S.STAMPS)) + " __HEAD_CID__").encode()
        stamps = {**dict.fromkeys(S.STAMPS, "fixture"), "MAINTAINER_DID": F.DID, "PUBLISHER_GATEWAY": F.ORIGIN}
        self.native = NativeFixture(self)
        stage = self.root / "initial-stage"
        self.native.stage_inputs([], "0.8.0-alpha.3", stage, F.PLATFORM)
        native_cids = {name: S.unixfs_metadata_cid((stage / "artifacts" / name).read_bytes())
            for name in json.loads((stage / "assembly.json").read_bytes())["files"]}
        self.put(self.root / "native-cids.json", S.json_bytes(native_cids))
        self.native.attach_input_cids(stage, self.root / "native-cids.json", F.PLATFORM)
        generated = "components-" + F.PLATFORM + ".json"
        full_cids = {**native_cids, generated: S.unixfs_metadata_cid((stage / "artifacts" / generated).read_bytes())}
        self.put(self.root / "cids.json", S.json_bytes(full_cids))
        self.put(self.root / "stamps.json", S.json_bytes(stamps))
        self.args.unsigned_v3.rmdir()
        self.native.signing_input(stage, self.root / "cids.json", self.root / "stamps.json", "canary",
            self.args.unsigned_v3, F.PLATFORM, None, None)
        manifest = S.parse_json((self.args.unsigned_v3 / "signing-input.json").read_bytes())
        release = manifest["release"]
        release_bytes = self.envelope(release)
        release_cid = S.unixfs_metadata_cid(release_bytes)
        head = {"schema": "elastos.release.head/v1", "channel": "canary", "version": "0.8.0-alpha.3",
            "latest_release_cid": release_cid, "release_sha256": S.sha256(release_bytes),
            "updated_at": manifest["head"]["updated_at"], "signer_did": F.DID, "prev_head_cid": None}
        self.public = {"release.json": release_bytes, "release-head.json": self.envelope(head),
            "install.sh": S.render_installer(self.template, stamps, F.DID)}
        self.put(self.args.publish_state, S.json_bytes({"publisher_did": F.DID, "last_release_cid": release_cid,
            "last_head_cid": S.unixfs_metadata_cid(self.public["release-head.json"]),
            "last_version": "0.8.0-alpha.3", "last_published_at": head["updated_at"]}))
        self.put(self.args.runtime_import, S.json_bytes({"sha256": S.sha256(n4_binary), "size": len(n4_binary),
            "cid": S.unixfs_metadata_cid(n4_binary)}))

    def put(self, path, data, mode=0o600):
        path.write_bytes(data)
        path.chmod(mode)

    def envelope(self, payload):
        return S.json_bytes({"payload": payload, "signature": "0" * 128, "signer_did": F.DID})

    def receipt(self, root, version, **extra):
        record = {"source": self.source, "platform": F.PLATFORM, "version": version,
            "files": {p.relative_to(root).as_posix(): N.file_record(p) for p in root.rglob("*")
                if p.is_file() and p.name != "platform-input.json"}, **extra}
        self.put(root / "platform-input.json", S.json_bytes(record))

    def run_finalize(self, **kwargs):
        return F.finalize(self.args, self.native, S, ROOT, fetch=self.public.__getitem__,
            verify_signature=lambda data, *unused: S.parse_json(data)["payload"], **kwargs)

    def test_original_receipts_runtime_support_cids_and_installer_are_preserved(self):
        originals = {root: (root / "platform-input.json").read_bytes()
            for root in (self.args.n3, self.args.n4, self.args.finalized_n3)}
        old_umask = os.umask(0)
        try:
            proof = self.run_finalize()
        finally:
            os.umask(old_umask)
        output = self.args.output / "unsigned-V4"
        before = S.parse_json((self.args.unsigned_v3 / "signing-input.json").read_bytes())
        after = S.parse_json((output / "signing-input.json").read_bytes())
        self.assertEqual(after["version"], "0.8.0-alpha.4")
        self.assertEqual(after["head"]["prev_head_cid"], proof["committed_v3_receipt"]["last_head_cid"])
        self.assertEqual(after["release"]["prev_release_cid"], proof["committed_v3_receipt"]["last_release_cid"])
        for name, pin in before["files"].items():
            if name != F.RUNTIME:
                self.assertEqual(after["files"][name], pin)
                self.assertEqual((output / name).read_bytes(), (self.args.unsigned_v3 / name).read_bytes())
        self.assertEqual((output / F.RUNTIME).read_bytes(), (self.args.n4 / "artifacts" / F.RUNTIME).read_bytes())
        for root, data in originals.items():
            self.assertEqual((root / "platform-input.json").read_bytes(), data)
        self.assertFalse((output / "V4-finalization.json").exists())
        self.assertEqual((self.args.output / "V4-finalization.json").stat().st_mode & 0o777, 0o600)
        self.assertEqual((self.args.output / "v3-publish-state.json").read_bytes(), self.args.publish_state.read_bytes())

    def test_capacity_refusal_leaves_no_output_or_partial(self):
        with self.assertRaisesRegex(ValueError, "15%"):
            self.run_finalize(measure=lambda unused: SimpleNamespace(total=1000, free=151))
        self.assertFalse(self.args.output.exists())
        self.assertEqual(list(self.root.glob(".v4-finalize-*")), [])

    def test_ci_pair_wrong_source_runtime_or_unexported_models_refuse(self):
        path = self.root / "receipts/native-pair.json"
        record = S.parse_json(path.read_bytes())
        for field, value in (("schema", "different/v1"), ("source_ci_run", "1"),
                             ("models_exported", False), ("publisher_did", "wrong")):
            self.put(path, S.json_bytes({**record, field: value}))
            with self.assertRaisesRegex(ValueError, "CI native pair"):
                self.run_finalize()
        changed = copy.deepcopy(record)
        changed["native"][1]["binary_sha256"] = "f" * 64
        self.put(path, S.json_bytes(changed))
        with self.assertRaisesRegex(ValueError, "CI Runtime"):
            self.run_finalize()

    def test_checkout_changes_at_completion_refuse(self):
        with patch.object(F, "check_checkouts", side_effect=ValueError("checkout commit differs")), \
                self.assertRaisesRegex(ValueError, "checkout commit"):
            self.run_finalize()
        self.assertFalse(self.args.output.exists())

    def test_identity_nested_origin_and_support_receipt_mismatches_refuse(self):
        path = self.args.n4 / "platform-input.json"
        original = path.read_bytes()
        for change in ({"version": "0.8.0-alpha.5"}, {"model_finalization": {}},
                {"source": {**self.source, "tree": "c" * 40}}):
            self.put(path, S.json_bytes({**S.parse_json(original), **change}))
            with self.assertRaises(ValueError): self.run_finalize()
        self.put(path, original)
        self.put(self.args.n4 / "support-input.json", b"{}")
        self.receipt(self.args.n4, "0.8.0-alpha.4", support_origin={})
        with self.assertRaisesRegex(ValueError, "support receipt"):
            self.run_finalize()

    def test_runtime_import_pin_and_raw_cid_mismatches_refuse(self):
        record = S.parse_json(self.args.runtime_import.read_bytes())
        for change in ({"size": record["size"] + 1}, {"sha256": "f" * 64}, {"cid": S.raw_cid(b"wrong")}):
            self.put(self.args.runtime_import, S.json_bytes({**record, **change}))
            with self.assertRaises(ValueError): self.run_finalize()

    def test_actual_seed_predecessor_and_public_v3_mismatches_refuse(self):
        record = S.parse_json(self.args.publish_state.read_bytes())
        self.put(self.args.publish_state, S.json_bytes({**record, "last_version": "0.8.0-alpha.2"}))
        with self.assertRaisesRegex(ValueError, "committed V3"):
            self.run_finalize()
        self.put(self.args.publish_state, S.json_bytes(record))
        self.public["install.sh"] += b"changed"
        with self.assertRaisesRegex(ValueError, "installer"):
            self.run_finalize()

    def test_failed_signature_refuses_before_output(self):
        with self.assertRaisesRegex(ValueError, "signature"):
            F.finalize(self.args, self.native, S, ROOT, fetch=self.public.__getitem__,
                verify_signature=lambda *unused: F.require(False, "signature verification failed"))
        self.assertFalse(self.args.output.exists())

    def test_envelope_refuses_wrong_signer_and_uses_exact_signature_domain(self):
        wrong = S.parse_json(self.public["release.json"])
        wrong["signer_did"] = "wrong"
        with patch.object(F.subprocess, "run") as command, self.assertRaisesRegex(ValueError, "signature envelope"):
            F.verify_envelope(S.json_bytes(wrong), "elastos.release.v1", S, self.args.openssl, self.root)
        command.assert_not_called()
        data = self.public["release.json"]
        def command(argv, **kwargs):
            self.assertEqual((kwargs["cwd"] / "digest").read_bytes(),
                S.signature_digest("elastos.release.v1", S.json_bytes(S.parse_json(data)["payload"])))
            self.assertIn("default", argv)
            self.assertTrue(kwargs["check"])
            self.assertEqual(kwargs["timeout"], 20)
            self.assertEqual(kwargs["stdin"], subprocess.DEVNULL)
            return subprocess.CompletedProcess(argv, 0, b"", b"")
        with patch.object(F.subprocess, "run", side_effect=command):
            F.verify_envelope(data, "elastos.release.v1", S, self.args.openssl, self.root)
        self.assertEqual(list(self.root.glob("signature-*")), [])

    def test_real_openssl3_refuses_zero_signature_without_private_key(self):
        openssl = Path("/opt/homebrew/opt/openssl@3/bin/openssl")
        if not openssl.exists():
            self.skipTest("qualified local OpenSSL 3 unavailable")
        reply = subprocess.run([str(openssl), "version"], capture_output=True, check=True, timeout=20)
        self.assertTrue(reply.stdout.startswith(b"OpenSSL 3."))
        for domain in ("elastos.release.v1", "elastos.release.head.v1"):
            with self.assertRaises(subprocess.CalledProcessError):
                F.verify_envelope(self.public["release.json"], domain, S, openssl.resolve(), self.root)
        self.assertEqual(list(self.root.glob("signature-*")), [])

    def test_catalogue_head_mismatch_refuses(self):
        self.components["model_catalog"]["head_cid"] = S.raw_cid(b"wrong")
        self.put(self.args.finalized_n3 / "components.json", S.json_bytes(self.components))
        self.receipt(self.args.finalized_n3, "0.8.0-alpha.3", model_finalization={
            "publisher_did": F.DID, "native_receipt_sha256": N.digest(self.args.n3 / "platform-input.json")})
        with self.assertRaisesRegex(ValueError, "catalogue binding"):
            self.run_finalize()

    def test_symlink_hardlink_existing_and_nested_outputs_refuse(self):
        file = self.args.n4 / "artifacts" / F.RUNTIME
        old = file.read_bytes()
        file.unlink()
        file.symlink_to(self.args.n3 / "artifacts" / F.RUNTIME)
        with self.assertRaises(ValueError): self.run_finalize()
        file.unlink()
        self.put(file, old, 0o700)
        os.link(file, self.root / "hardlink")
        with self.assertRaises(ValueError): self.run_finalize()
        (self.root / "hardlink").unlink()
        self.args.output.mkdir()
        with self.assertRaisesRegex(ValueError, "fresh output"): self.run_finalize()
        self.args.output.rmdir()
        self.args.output = self.args.n4 / "nested"
        with self.assertRaisesRegex(ValueError, "outside inputs"): self.run_finalize()

    def test_writable_parent_refuses_private_output_directory(self):
        parent = self.root / "shared-parent"
        parent.mkdir(mode=0o700)
        output_parent = parent / "private-output-parent"
        output_parent.mkdir(mode=0o700)
        parent.chmod(0o770)
        self.args.output = output_parent / "V4-finalized"
        with self.assertRaisesRegex(ValueError, "path ancestry"):
            self.run_finalize()

    def test_runtime_change_during_copy_cleans_all_scratch(self):
        original_copy = shutil.copyfile
        def copying(source, destination, *args, **kwargs):
            result = original_copy(source, destination, *args, **kwargs)
            if Path(source) == self.args.n4 / "artifacts" / F.RUNTIME:
                with Path(source).open("ab") as stream: stream.write(b"mutated")
            return result
        with patch.object(F.shutil, "copyfile", side_effect=copying), self.assertRaises(ValueError):
            self.run_finalize()
        self.assertFalse(self.args.output.exists())
        self.assertEqual(list(self.root.glob(".v4-finalize-*")), [])

    def test_public_head_change_before_promotion_refuses(self):
        count = 0
        def fetch(name):
            nonlocal count
            count += 1
            return self.public[name] + (b"changed" if count > 3 else b"")
        with self.assertRaisesRegex(ValueError, "public V3 changed"):
            F.finalize(self.args, self.native, S, ROOT, fetch=fetch,
                verify_signature=lambda data, *unused: S.parse_json(data)["payload"])
        self.assertFalse(self.args.output.exists())


if __name__ == "__main__":
    unittest.main()
