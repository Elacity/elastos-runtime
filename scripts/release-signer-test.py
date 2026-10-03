#!/usr/bin/env python3
"""Public-data/fake-backend refusal tests. No private keys or real signing."""

import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

SOURCE = Path(__file__).with_name("release-signer.py").resolve()
spec = importlib.util.spec_from_file_location("release_signer", SOURCE)
S = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = S
spec.loader.exec_module(S)

# RFC 8032 public verification vector. Only its public key is present here.
PUBLIC = bytes.fromhex("d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a")
DID = S.public_did(PUBLIC)
COMMIT, TREE, MAIN, SCRIPTS, TAG = (digit * 40 for digit in "abcde")
DEVELOP = "1" * 40


class FakeBackend:
    def __init__(self):
        self.calls = []
        self.signature = bytes(64)
        self.verified = True
        self.public = PUBLIC

    def public_key(self):
        self.calls.append("public")
        return self.public

    def sign(self, digest):
        self.calls.append(("sign", digest))
        return self.signature

    def verify(self, digest, signature):
        self.calls.append(("verify", digest, signature))
        return self.verified

    def close(self):
        self.calls.append("close")


class SignerTests(unittest.TestCase):
    def setUp(self):
        # These directories are removed by unittest cleanup, including refusals.
        self.temp = tempfile.TemporaryDirectory(dir=SOURCE.parent.parent)
        self.addCleanup(self.temp.cleanup)
        self.base = Path(self.temp.name).resolve()
        self.root = self.base / "candidate"
        self.root.mkdir()
        self.snapshot = self.base / "snapshot"
        self.snapshot.mkdir(mode=0o700)
        self.marker = self.base / "executed"
        self.binary = f"#!/bin/sh\ncat /custodian/unopened-signing.pem > {self.marker}\n".encode()
        (self.root / "elastos").write_bytes(self.binary)
        (self.root / "elastos").chmod(0o755)
        self.components = S.json_bytes({"schema": "elastos.components/v1", "external": {}})
        (self.root / "components.json").write_bytes(self.components)
        self.template = ("#!/bin/sh\n" + "\n".join(f'{name}="__{name}__"' for name in sorted(S.STAMPS | {"HEAD_CID"})) + f"\ncat /custodian/unopened-signing.pem > {self.marker}\n").encode()
        self.blob_oid = hashlib.sha1(b"blob " + str(len(self.template)).encode() + b"\0" + self.template).hexdigest()
        self.stamps = {"MAINTAINER_DID": DID, "SOURCE_CONNECT_TICKET": "public-ticket",
                       "PUBLISHER_GATEWAY": "https://publisher.invalid", "PUBLISHER_NODE_ID": "public-node", "IPNS_NAME": "public-ipns"}
        rendered = S.render_installer(self.template, self.stamps, DID)
        def record(data):
            return {"sha256": S.sha256(data), "size": len(data), "cid": S.raw_cid(data)}
        self.manifest = {"source": {"commit": COMMIT, "tree": TREE}, "version": "1.2.3", "channel": "stable",
                         "files": {"elastos": record(self.binary), "components.json": record(self.components)},
                         "installer": {"blob_oid": self.blob_oid, "stamps": self.stamps},
                         "release": {"schema": "elastos.release/v1", "source": {"commit": COMMIT, "tree": TREE},
                                     "version": "1.2.3", "channel": "stable", "released_at": 1, "prev_release_cid": None,
                                     "platforms": {"x86_64-linux": {"binary": {k: record(self.binary)[k] for k in ("cid", "sha256")},
                                                                    "components": {k: record(self.components)[k] for k in ("cid", "sha256")}}},
                                     "installer_sha256": S.sha256(rendered)},
                         "head": {"updated_at": 2, "prev_head_cid": None}}
        self.policy = {"repository": S.REPOSITORY, "tag": "v1.2.3", "tag_oid": COMMIT, "commit": COMMIT,
                       "tree": TREE, "version": "1.2.3", "channel": "stable", "publisher_did": DID,
                       "max_file_bytes": 1024 * 1024, "max_snapshot_bytes": 4 * 1024 * 1024,
                       "key_path": "/custodian/unopened-signing.pem"}
        self.approve_manifest()
        prefix = f"/repos/{S.REPOSITORY}"
        self.api = {
            f"{prefix}/git/ref/tags/v1.2.3": {"ref": "refs/tags/v1.2.3", "object": {"type": "commit", "sha": COMMIT}},
            f"{prefix}/git/commits/{COMMIT}": {"sha": COMMIT, "tree": {"sha": TREE}},
            f"{prefix}/git/ref/heads/main": {"ref": "refs/heads/main", "object": {"type": "commit", "sha": MAIN}},
            f"{prefix}/git/commits/{MAIN}": {"sha": MAIN, "tree": {"sha": "f" * 40}},
            f"{prefix}/compare/{COMMIT}...{MAIN}?per_page=1": {"status": "ahead", "behind_by": 0, "ahead_by": 1,
                "base_commit": {"sha": COMMIT}, "merge_base_commit": {"sha": COMMIT}},
            f"{prefix}/git/trees/{TREE}": {"sha": TREE, "truncated": False, "tree": [{"path": "scripts", "type": "tree", "mode": "040000", "sha": SCRIPTS}]},
            f"{prefix}/git/trees/{SCRIPTS}": {"sha": SCRIPTS, "truncated": False, "tree": [{"path": "install.sh", "type": "blob", "mode": "100755", "sha": self.blob_oid}]},
            f"{prefix}/git/blobs/{self.blob_oid}": {"sha": self.blob_oid, "encoding": "base64", "size": len(self.template), "content": base64.b64encode(self.template).decode()},
        }
        self.requests = []
        self.no_process = mock.patch.object(S.subprocess, "run", side_effect=AssertionError("real subprocess refused in tests"))
        self.no_process.start()
        self.addCleanup(self.no_process.stop)
        # Unit fixtures use a deterministic healthy volume. The dedicated quota
        # case below overrides this with a full volume to prove the real guard.
        disk = mock.patch.object(S.shutil, "disk_usage", return_value=SimpleNamespace(
            total=100 * 1024**3, free=50 * 1024**3))
        disk.start()
        self.addCleanup(disk.stop)

    def approve_manifest(self):
        data = S.json_bytes(self.manifest)
        (self.root / "signing-input.json").write_bytes(data)
        self.policy["manifest_sha256"] = S.sha256(data)

    def fetch(self, path):
        self.requests.append(path)
        if path not in self.api:
            raise ValueError("remote object unavailable")
        return copy.deepcopy(self.api[path])

    def prepare(self):
        return S.prepare(self.policy, self.root, "signing-input.json", self.fetch, self.snapshot)

    def test_snapshot_signs_only_frozen_bytes_and_binds_installer_release_head(self):
        prepared = self.prepare()
        (self.root / "elastos").write_bytes(b"changed after snapshot")
        (self.root / "components.json").write_bytes(b"changed after snapshot")
        backend = FakeBackend()
        publication = dict(S.sign_publication(prepared, backend))
        self.assertEqual(publication["elastos"].read_bytes(), self.binary)
        release = S.parse_json(publication["release.json"])
        head = S.parse_json(publication["release-head.json"])
        self.assertEqual(release["payload"]["installer_sha256"], S.sha256(publication["install.sh"].read_bytes()))
        self.assertEqual(head["payload"]["release_sha256"], S.sha256(publication["release.json"]))
        self.assertEqual(head["payload"]["latest_release_cid"], S.unixfs_metadata_cid(publication["release.json"]))
        self.assertIn(b'HEAD_CID=""', publication["install.sh"].read_bytes())
        self.assertFalse(self.marker.exists())
        self.assertEqual(len([call for call in backend.calls if isinstance(call, tuple) and call[0] == "sign"]), 2)

    def test_annotated_tag_and_identical_main_are_supported(self):
        prefix = f"/repos/{S.REPOSITORY}"
        self.policy["tag_oid"] = TAG
        self.api[f"{prefix}/git/ref/tags/v1.2.3"]["object"] = {"type": "tag", "sha": TAG}
        self.api[f"{prefix}/git/tags/{TAG}"] = {"sha": TAG, "tag": "v1.2.3", "object": {"type": "commit", "sha": COMMIT}}
        self.api[f"{prefix}/git/ref/heads/main"]["object"]["sha"] = COMMIT
        self.api[f"{prefix}/compare/{COMMIT}...{COMMIT}?per_page=1"] = {"status": "identical", "behind_by": 0, "ahead_by": 0,
                                                                       "base_commit": {"sha": COMMIT}, "merge_base_commit": {"sha": COMMIT}}
        self.prepare()
        self.assertTrue(any("/git/tags/" in path for path in self.requests))

    def test_local_only_and_moved_tags_are_refused(self):
        path = f"/repos/{S.REPOSITORY}/git/ref/tags/v1.2.3"
        original = self.api.pop(path)
        with self.assertRaisesRegex(ValueError, "unavailable"):
            self.prepare()
        self.api[path] = original
        original["object"]["sha"] = MAIN
        with self.assertRaisesRegex(ValueError, "tag moved"):
            self.prepare()

    def canary_policy(self):
        self.policy["channel"] = "canary"
        self.policy["develop_oid"] = DEVELOP
        del self.policy["tag"]
        del self.policy["tag_oid"]
        self.manifest["channel"] = self.manifest["release"]["channel"] = "canary"
        self.approve_manifest()
        prefix = f"/repos/{S.REPOSITORY}"
        del self.api[f"{prefix}/git/ref/tags/v1.2.3"]
        self.api[f"{prefix}/git/ref/heads/develop"] = {
            "ref": "refs/heads/develop", "object": {"type": "commit", "sha": DEVELOP}}
        self.api[f"{prefix}/git/commits/{DEVELOP}"] = {"sha": DEVELOP, "tree": {"sha": "f" * 40}}
        self.api[f"{prefix}/compare/{COMMIT}...{DEVELOP}?per_page=1"] = {
            "status": "ahead", "behind_by": 0, "ahead_by": 1,
            "base_commit": {"sha": COMMIT}, "merge_base_commit": {"sha": COMMIT}}

    def test_canary_merged_develop_source_preserves_publication_contract(self):
        self.canary_policy()
        # Main has no ancestry proof for this develop-only candidate.
        del self.api[f"/repos/{S.REPOSITORY}/compare/{COMMIT}...{MAIN}?per_page=1"]
        publication = dict(S.sign_publication(self.prepare(), FakeBackend()))
        release = S.parse_json(publication["release.json"])["payload"]
        head = S.parse_json(publication["release-head.json"])["payload"]
        self.assertEqual(release["source"], {"commit": COMMIT, "tree": TREE})
        self.assertEqual(release["channel"], "canary")
        self.assertEqual(head["channel"], "canary")
        self.assertFalse(any("/tags/" in path or "/heads/main" in path for path in self.requests))
        self.assertFalse(self.marker.exists())

    def test_canary_identical_develop_head_is_supported(self):
        self.canary_policy()
        prefix = f"/repos/{S.REPOSITORY}"
        self.policy["develop_oid"] = COMMIT
        self.api[f"{prefix}/git/ref/heads/develop"]["object"]["sha"] = COMMIT
        self.api[f"{prefix}/compare/{COMMIT}...{COMMIT}?per_page=1"] = {
            "status": "identical", "behind_by": 0, "ahead_by": 0,
            "base_commit": {"sha": COMMIT}, "merge_base_commit": {"sha": COMMIT}}
        self.prepare()

    def test_canary_local_only_commit_or_develop_ref_is_refused(self):
        self.canary_policy()
        for path in (f"/repos/{S.REPOSITORY}/git/commits/{COMMIT}",
                     f"/repos/{S.REPOSITORY}/git/ref/heads/develop"):
            with self.subTest(path=path):
                original = self.api.pop(path)
                with self.assertRaisesRegex(ValueError, "unavailable"):
                    self.prepare()
                self.api[path] = original
        self.assertEqual(list(self.snapshot.iterdir()), [])

    def test_canary_off_develop_or_malformed_comparison_is_refused(self):
        self.canary_policy()
        path = f"/repos/{S.REPOSITORY}/compare/{COMMIT}...{DEVELOP}?per_page=1"
        original = copy.deepcopy(self.api[path])
        for changes in ({"status": "behind"}, {"status": "diverged"}, {"behind_by": 1}, {"behind_by": False},
                        {"ahead_by": -1}, {"ahead_by": True}, {"status": "identical"}, {"ahead_by": 0},
                        {"merge_base_commit": {"sha": DEVELOP}}, {"base_commit": {"sha": DEVELOP}},
                        {"base_commit": None}, {"merge_base_commit": None}):
            with self.subTest(changes=changes):
                self.api[path] = {**original, **changes}
                with self.assertRaises(ValueError):
                    self.prepare()
        self.assertEqual(list(self.snapshot.iterdir()), [])

    def test_canary_moved_wrong_or_malformed_ref_and_tree_are_refused(self):
        self.canary_policy()
        prefix = f"/repos/{S.REPOSITORY}"
        ref = self.api[f"{prefix}/git/ref/heads/develop"]
        for changes in ({"ref": "refs/heads/main"}, {"object": {"type": "commit", "sha": MAIN}},
                        {"object": {"type": "tag", "sha": DEVELOP}}, {"object": {"type": "commit", "sha": "bad"}}):
            with self.subTest(changes=changes), mock.patch.dict(ref, changes):
                with self.assertRaises(ValueError):
                    self.prepare()
        self.api[f"{prefix}/git/commits/{COMMIT}"]["tree"]["sha"] = MAIN
        with self.assertRaisesRegex(ValueError, "source tree"):
            self.prepare()
        self.api[f"{prefix}/git/commits/{COMMIT}"]["tree"]["sha"] = TREE
        self.api[f"{prefix}/git/commits/{DEVELOP}"]["sha"] = MAIN
        with self.assertRaisesRegex(ValueError, "typed develop"):
            self.prepare()

    def test_canary_requires_full_develop_pin_and_fixed_branch(self):
        self.canary_policy()
        for value in (None, True, "1" * 7):
            with self.subTest(value=value), mock.patch.dict(self.policy, {"develop_oid": value}):
                with self.assertRaisesRegex(ValueError, "full Git"):
                    self.prepare()
        # Candidate-controlled branch names cannot redirect the canonical proof.
        self.policy["branch"] = "feature/local-only"
        self.api[f"/repos/{S.REPOSITORY}/compare/{COMMIT}...{DEVELOP}?per_page=1"]["status"] = "diverged"
        with self.assertRaisesRegex(ValueError, "outside develop"):
            self.prepare()

    def test_stable_develop_only_candidate_keeps_tag_and_main_gate(self):
        self.canary_policy()
        self.policy["channel"] = "stable"
        self.manifest["channel"] = self.manifest["release"]["channel"] = "stable"
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "tag must match"):
            self.prepare()
        self.policy.update(tag="v1.2.3", tag_oid=COMMIT, branch="develop")
        prefix = f"/repos/{S.REPOSITORY}"
        self.api[f"{prefix}/git/ref/tags/v1.2.3"] = {
            "ref": "refs/tags/v1.2.3", "object": {"type": "commit", "sha": COMMIT}}
        self.api[f"{prefix}/compare/{COMMIT}...{MAIN}?per_page=1"]["status"] = "diverged"
        with self.assertRaisesRegex(ValueError, "outside main"):
            self.prepare()
        self.assertIn(f"{prefix}/git/ref/heads/main", self.requests)
        self.assertFalse(any("/heads/develop" in path for path in self.requests))

    def test_canary_manifest_and_release_channel_mismatch_are_refused(self):
        self.canary_policy()
        self.manifest["channel"] = "stable"
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "source/version/channel"):
            self.prepare()
        self.manifest["channel"] = "canary"
        self.manifest["release"]["channel"] = "stable"
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "release identity"):
            self.prepare()

    def test_off_main_diverged_or_malformed_comparison_is_refused(self):
        path = f"/repos/{S.REPOSITORY}/compare/{COMMIT}...{MAIN}?per_page=1"
        original = copy.deepcopy(self.api[path])
        for changes in ({"status": "behind"}, {"status": "diverged"}, {"behind_by": 1}, {"behind_by": False},
                        {"ahead_by": -1}, {"status": "identical"}, {"merge_base_commit": {"sha": MAIN}}, {"base_commit": {"sha": MAIN}}):
            with self.subTest(changes=changes):
                self.api[path] = {**original, **changes}
                with self.assertRaises(ValueError):
                    self.prepare()

    def test_malformed_versions_refs_and_tag_targets_are_refused(self):
        original = copy.deepcopy(self.policy)
        for changes in ({"repository": "other/repo"}, {"version": "01.2.3"}, {"version": "1.2.3-01"},
                        {"tag": "../main"}, {"tag": "v1.2.3/other"}, {"commit": "a" * 7}, {"tree": True}, {"channel": "nightly"}):
            with self.subTest(changes=changes):
                self.policy = {**original, **changes}
                with self.assertRaises(ValueError):
                    self.prepare()
        self.policy = original
        tag = self.api[f"/repos/{S.REPOSITORY}/git/ref/tags/v1.2.3"]
        for changes in ({"ref": "refs/heads/main"}, {"object": {"type": "blob", "sha": COMMIT}}, {"object": {"type": "commit", "sha": "bad"}}):
            with mock.patch.dict(tag, changes):
                with self.assertRaises(ValueError):
                    self.prepare()

    def test_tag_cycle_and_tree_identity_tamper_are_refused(self):
        prefix = f"/repos/{S.REPOSITORY}"
        self.policy["tag_oid"] = TAG
        self.api[f"{prefix}/git/ref/tags/v1.2.3"]["object"] = {"type": "tag", "sha": TAG}
        self.api[f"{prefix}/git/tags/{TAG}"] = {"sha": TAG, "tag": "v1.2.3", "object": {"type": "tag", "sha": TAG}}
        with self.assertRaisesRegex(ValueError, "chain"):
            self.prepare()
        self.api[f"{prefix}/git/tags/{TAG}"]["object"] = {"type": "commit", "sha": COMMIT}
        self.api[f"{prefix}/git/commits/{COMMIT}"]["tree"]["sha"] = MAIN
        with self.assertRaisesRegex(ValueError, "tree"):
            self.prepare()

    def test_manifest_hash_is_checked_against_operator_approval(self):
        self.manifest["release"]["released_at"] += 1
        (self.root / "signing-input.json").write_bytes(S.json_bytes(self.manifest))
        with self.assertRaisesRegex(ValueError, "operator approval"):
            self.prepare()
        self.assertEqual(list(self.snapshot.iterdir()), [])

    def test_tampered_artifact_is_refused(self):
        (self.root / "elastos").write_bytes(b"x" * len(self.binary))
        with self.assertRaisesRegex(ValueError, "artifact differs"):
            self.prepare()

    def test_symlink_hardlink_directory_and_escape_are_refused(self):
        path = self.root / "elastos"
        path.unlink()
        path.symlink_to(self.root / "components.json")
        with self.assertRaises(OSError):
            self.prepare()
        for copied in self.snapshot.iterdir():
            copied.unlink()
        path.unlink()
        os.link(self.root / "components.json", path)
        with self.assertRaisesRegex(ValueError, "unlinked"):
            self.prepare()
        for copied in self.snapshot.iterdir():
            copied.unlink()
        path.unlink()
        path.mkdir()
        with self.assertRaisesRegex(ValueError, "unlinked"):
            self.prepare()
        for name in ("../elastos", "/elastos", "nested/../elastos", "nested//elastos", "nested\\elastos"):
            with self.subTest(name=name), self.assertRaisesRegex(ValueError, "unsafe"):
                S.relative_path(name)

    def test_artifact_and_total_quotas_and_free_space_fail_before_copy(self):
        for quota in ("max_file_bytes", "max_snapshot_bytes"):
            original = self.policy[quota]
            self.policy[quota] = 1
            with self.assertRaises(ValueError):
                self.prepare()
            self.assertEqual(list(self.snapshot.iterdir()), [])
            self.policy[quota] = original
        with mock.patch.object(S.shutil, "disk_usage", return_value=SimpleNamespace(total=1000, free=150)):
            with self.assertRaisesRegex(ValueError, "15 percent"):
                self.prepare()
        self.assertEqual(list(self.snapshot.iterdir()), [])

    def test_wrong_payload_source_version_channel_installer_or_artifact_is_refused(self):
        original = copy.deepcopy(self.manifest)
        for field, value in (("source", {"commit": MAIN, "tree": TREE}), ("version", "9.9.9"), ("channel", "canary"), ("installer_sha256", "0" * 64)):
            with self.subTest(field=field):
                self.manifest = copy.deepcopy(original)
                self.manifest["release"][field] = value
                self.approve_manifest()
                with self.assertRaises(ValueError):
                    self.prepare()
                for path in self.snapshot.iterdir():
                    path.unlink()
        self.manifest = copy.deepcopy(original)
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"]["sha256"] = "0" * 64
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "reference differs"):
            self.prepare()

    def test_template_hash_tree_mode_and_injection_are_refused(self):
        blob_path = f"/repos/{S.REPOSITORY}/git/blobs/{self.blob_oid}"
        self.api[blob_path]["content"] = base64.b64encode(self.template + b"x").decode()
        with self.assertRaisesRegex(ValueError, "blob hash"):
            self.prepare()
        for stamp in ('$(touch /tmp/marker)', '`touch /tmp/marker`', '"; exit', "__HEAD_CID__", "x\ny"):
            with self.subTest(stamp=stamp), self.assertRaises(ValueError):
                S.render_installer(self.template, {**self.stamps, "IPNS_NAME": stamp}, DID)
        with self.assertRaises(ValueError):
            S.render_installer(self.template, {**self.stamps, "HEAD_CID": "circular-head"}, DID)

    def test_unbound_artifact_is_refused(self):
        extra = b"unadvertised"
        (self.root / "extra").write_bytes(extra)
        self.manifest["files"]["extra"] = {"sha256": S.sha256(extra), "size": len(extra), "cid": S.raw_cid(extra)}
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "unbound"):
            self.prepare()

    def test_public_did_signature_length_and_public_verification_refuse(self):
        prepared = self.prepare()
        for attribute, value, reason in (("public", bytes(32), "DID"), ("signature", bytes(63), "length"), ("verified", False, "verification")):
            backend = FakeBackend()
            setattr(backend, attribute, value)
            with self.subTest(attribute=attribute), self.assertRaisesRegex(ValueError, reason):
                S.sign_publication(prepared, backend)

    def test_json_canonical_encoding_domain_separation_and_refusals(self):
        data = {"z": "café", "a": [1, True, None, "\n"]}
        expected = b'{"a":[1,true,null,"\\n"],"z":"caf\xc3\xa9"}'
        self.assertEqual(S.json_bytes(data), expected)
        self.assertEqual(S.signature_digest("elastos.release.v1", expected), hashlib.sha256(b"elastos.release.v1\0" + expected).digest())
        for domain in ("elastos.catalog.v1", "", "elastos.release.v1\0evil"):
            with self.assertRaises(ValueError):
                S.signature_digest(domain, expected)
        for value in ({"float": 1.0}, {"nan": float("nan")}, {"integer": 2**64}, {"integer": -(2**63) - 1}):
            with self.assertRaises(ValueError):
                S.json_bytes(value)
        with self.assertRaises(ValueError):
            S.parse_json(b'{"a":1,"a":2}')
        with mock.patch.object(S, "MAX_JSON", 4), self.assertRaises(ValueError):
            S.parse_json(b'{"a":1}')

    def test_cancellation_and_wrong_confirmation_use_no_backend(self):
        prepared = self.prepare()
        backend = mock.Mock()
        for response in ("", "\n", "yes\n", "did:key:wrong\n"):
            self.assertFalse(S.confirmed(prepared, io.StringIO(response), io.StringIO()))
            backend.assert_not_called()
        self.assertTrue(S.confirmed(prepared, io.StringIO(DID + "\n"), io.StringIO()))

    def test_cli_cancellation_precedes_backend_and_preserves_output_absence(self):
        policy_path = self.base / "operator-policy.json"
        policy_path.write_bytes(S.json_bytes(self.policy))
        output_path = self.base / "publication"
        with mock.patch.object(sys, "argv", [str(SOURCE), "--policy", str(policy_path), "--input-root", str(self.root), "--output-root", str(output_path)]), \
             mock.patch.object(S, "pinned_tools"), mock.patch.object(S, "github_json", side_effect=self.fetch), \
             mock.patch.object(sys, "stdin", io.StringIO("\n")), mock.patch.object(sys, "stderr", io.StringIO()), \
             mock.patch.object(S, "OpenSSLBackend") as backend:
            with self.assertRaisesRegex(ValueError, "cancelled"):
                S.main()
            backend.assert_not_called()
        self.assertFalse(output_path.exists())
        self.assertFalse(any(path.name.startswith(".elastos-signing-") for path in self.base.iterdir()))

    def test_https_is_fixed_bounded_and_refuses_redirects(self):
        response = mock.Mock(status=301)
        connection = mock.Mock()
        connection.getresponse.return_value = response
        with mock.patch.object(S.http.client, "HTTPSConnection", return_value=connection) as factory, \
             mock.patch.dict(os.environ, {"HTTPS_PROXY": "https://attacker.invalid", "HTTP_PROXY": "http://attacker.invalid"}):
            with self.assertRaises(ValueError):
                S.github_json(f"/repos/{S.REPOSITORY}/git/ref/heads/main")
            self.assertEqual(factory.call_args.args[0], "api.github.com")
            self.assertEqual(factory.call_args.kwargs["timeout"], 20)
            connection.close.assert_called_once()
        with self.assertRaises(ValueError):
            S.github_json("https://attacker.invalid/data")
        response.status = 200
        response.getheader.return_value = "application/json"
        response.read.return_value = b'{}'
        with mock.patch.object(S.http.client, "HTTPSConnection", return_value=connection):
            self.assertEqual(S.github_json(f"/repos/{S.REPOSITORY}/git/ref/heads/main"), {})
            response.read.assert_called_once_with(S.MAX_JSON + 1)

    def test_tls_environment_overrides_are_refused_before_connection(self):
        for name in ("SSL_CERT_FILE", "SSL_CERT_DIR"):
            with mock.patch.dict(os.environ, {name: str(self.marker)}), mock.patch.object(S.http.client, "HTTPSConnection") as connection:
                with self.assertRaisesRegex(ValueError, "TLS environment"):
                    S.github_json(f"/repos/{S.REPOSITORY}/git/ref/heads/main")
                connection.assert_not_called()

    def test_swapped_ancestor_cannot_escape_held_input_root(self):
        nested = self.root / "nested"
        nested.mkdir()
        outside = self.base / "outside"
        outside.mkdir()
        public_marker = b"harmless outside marker"
        (outside / "file").write_bytes(public_marker)
        (nested / "file").write_bytes(b"candidate")
        record = {"size": len(public_marker), "sha256": S.sha256(public_marker), "cid": S.raw_cid(public_marker)}
        held = S.directory_fd(self.root)
        real_open = os.open
        swapped = False
        def swap_then_open(path, flags, *args, **kwargs):
            nonlocal swapped
            if path == "nested" and not swapped:
                nested.rename(self.root / "original-nested")
                nested.symlink_to(outside, target_is_directory=True)
                swapped = True
            return real_open(path, flags, *args, **kwargs)
        try:
            with mock.patch.object(S.os, "open", side_effect=swap_then_open):
                with self.assertRaises(OSError):
                    S.snapshot_artifact(Path("nested/file"), self.snapshot / "escaped", record, held)
            self.assertTrue(swapped)
            self.assertFalse((self.snapshot / "escaped").exists())
        finally:
            os.close(held)

    def test_backend_temporary_parent_ignores_candidate_tmpdir(self):
        # Exercise constructor through fake public OpenSSL output and fake key
        # metadata. No key file exists or is read.
        key_path = "/custodian/unopened-signing.pem"
        actual_lstat = Path.lstat
        actual_stat = Path.stat
        info = SimpleNamespace(st_mode=0o100600, st_uid=os.getuid(), st_nlink=1)
        def fake_lstat(path, *args, **kwargs):
            return info if str(path) in (key_path, "/custodian") else actual_lstat(path, *args, **kwargs)
        def fake_stat(path, *args, **kwargs):
            return info if str(path) == key_path else actual_stat(path, *args, **kwargs)
        der = bytes.fromhex("302a300506032b6570032100") + PUBLIC
        with mock.patch.dict(os.environ, {"TMPDIR": str(self.root), "TEMP": str(self.root), "TMP": str(self.root)}), \
             mock.patch.object(Path, "lstat", fake_lstat), mock.patch.object(Path, "stat", fake_stat), \
             mock.patch.object(S.OpenSSLBackend, "run", side_effect=[b"OpenSSL 3.6.3", der]):
            backend = S.OpenSSLBackend({"openssl": {"path": "/custodian/pinned/openssl"}, "key_path": key_path}, self.root, self.snapshot)
            try:
                self.assertEqual(backend.root.parent, self.snapshot)
                self.assertFalse(backend.root.is_relative_to(self.root))
                self.assertEqual(backend.public_key(), PUBLIC)
                self.assertFalse(self.marker.exists())
            finally:
                backend.close()

    def test_openssl_invocation_uses_only_pinned_absolute_path_clean_env_and_trusted_cwd(self):
        backend = S.OpenSSLBackend.__new__(S.OpenSSLBackend)
        backend.executable = "/custodian/pinned/openssl"
        backend.root = self.snapshot
        with mock.patch.object(S.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout=b"public", stderr=b"")) as process:
            self.assertEqual(backend.run(["version"]), b"public")
            self.assertEqual(process.call_args.args[0], ["/custodian/pinned/openssl", "version"])
            options = process.call_args.kwargs
            self.assertEqual(options["cwd"], self.snapshot)
            self.assertEqual(options["env"], {"OPENSSL_CONF": "/dev/null", "LANG": "C"})
            self.assertFalse(options["shell"])
            self.assertEqual(options["timeout"], 20)

    def test_tool_pins_refuse_hash_path_and_unprotected_policy(self):
        tool = {"path": str(SOURCE), "sha256": "0" * 64}
        self.policy.update(tool=tool, python={"path": str(Path(sys.executable).resolve()), "sha256": "0" * 64}, openssl={"path": "/custodian/openssl", "sha256": "0" * 64})
        with self.assertRaisesRegex(ValueError, "hash differs"):
            S.pinned_tools(self.policy, self.root)
        self.policy["tool"]["path"] = str(self.root / "elastos")
        with self.assertRaises(ValueError):
            S.pinned_tools(self.policy, self.root)
        path = self.base / "operator-policy.json"
        path.write_bytes(b'{}')
        path.chmod(0o666)
        with self.assertRaisesRegex(ValueError, "unprotected"):
            S.regular_bytes(path, S.MAX_JSON, trusted=True)

    def test_cid_supported_forms_preserve_approved_dag_pb_and_refuse_bad_encoding(self):
        root_digest = bytes.fromhex("12" * 32)
        v0 = S.base58_encode(b"\x12\x20" + root_digest)
        v1 = "b" + base64.b32encode(b"\x01\x70\x12\x20" + root_digest).decode().lower().rstrip("=")
        for text in (v0, v1):
            self.assertEqual(S.cid_info(text), (0x70, root_digest))
        self.assertEqual(S.cid_info(S.raw_cid(self.binary)), (0x55, hashlib.sha256(self.binary).digest()))
        for text in ("QmBad", v1.upper(), v1 + "=", "z" + v0, "bafk", "", "b" + base64.b32encode(b"\x81\x00\x70\x12\x20" + root_digest).decode().lower().rstrip("="),
                     "b" + base64.b32encode(b"\x01\x71\x12\x20" + root_digest).decode().lower().rstrip("=")):
            with self.subTest(text=text), self.assertRaises(ValueError):
                S.cid_info(text)
        self.manifest["files"]["elastos"]["cid"] = v0
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"]["cid"] = v0
        self.approve_manifest()
        prepared = self.prepare()
        publication = dict(S.sign_publication(prepared, FakeBackend()))
        self.assertEqual(S.parse_json(publication["release.json"])["payload"]["platforms"]["x86_64-linux"]["binary"]["cid"], v0)
        self.assertNotEqual(root_digest, hashlib.sha256(self.binary).digest())
        (self.root / "elastos").write_bytes(b"x" * len(self.binary))
        fresh_snapshot = self.base / "tampered-snapshot"
        fresh_snapshot.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, "artifact differs"):
            S.prepare(self.policy, self.root, "signing-input.json", self.fetch, fresh_snapshot)

    def test_raw_cid_mismatch_and_changed_approved_cid_reference_are_refused(self):
        self.manifest["files"]["elastos"]["cid"] = S.raw_cid(b"other bytes")
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "raw CID differs"):
            self.prepare()
        for path in self.snapshot.iterdir():
            path.unlink()
        self.manifest["files"]["elastos"]["cid"] = S.raw_cid(self.binary)
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"]["cid"] = S.base58_encode(b"\x12\x20" + bytes(32))
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "reference differs"):
            self.prepare()

    def current_shape(self):
        provider = b"native provider inert bytes"
        metadata = b"provider capsule inert tarball"
        catalog = S.json_bytes({"schema": "elastos.model-catalog/v1", "payload": {"models": []}, "signature": "public fixture"})
        def add(name, data):
            (self.root / name).write_bytes(data)
            record = {"sha256": S.sha256(data), "size": len(data), "cid": S.raw_cid(data)}
            self.manifest["files"][name] = record
            return record
        provider_record = add("localhost-provider-linux-amd64", provider)
        metadata_record = add("localhost-provider-capsule-metadata.tar.gz", metadata)
        catalog_record = add("model-catalog.json", catalog)
        def external_ref(record, name):
            return {"cid": record["cid"], "checksum": "sha256:" + record["sha256"], "size": record["size"],
                    "release_path": name, "install_path": "bin/localhost-provider"}
        components = {"schema": "elastos.components/v1", "capsules": {}, "external": {"localhost-provider": {
            "platforms": {"linux-amd64": external_ref(provider_record, "localhost-provider-linux-amd64")},
            "capsule_metadata": {"platforms": {"*": external_ref(metadata_record, "localhost-provider-capsule-metadata.tar.gz")}}}},
            "model_catalog": {"head_cid": catalog_record["cid"]}}
        data = S.json_bytes(components)
        components_record = add("components.json", data)
        platform = self.manifest["release"]["platforms"]["x86_64-linux"]
        platform["components"] = copy.deepcopy(components_record)
        platform["binary"] = copy.deepcopy(self.manifest["files"]["elastos"])
        self.approve_manifest()
        return components

    def test_current_release_sizes_external_checksum_paths_and_catalog_are_bound(self):
        self.current_shape()
        prepared = self.prepare()
        self.assertEqual(len(prepared.files), 6)
        self.assertFalse(self.marker.exists())

    def test_release_size_mismatch_and_null_are_refused(self):
        for size in (0, None, True):
            self.manifest["release"]["platforms"]["x86_64-linux"]["binary"]["size"] = size
            self.approve_manifest()
            with self.subTest(size=size), self.assertRaises(ValueError):
                self.prepare()
            for path in self.snapshot.iterdir():
                path.unlink()

    def test_external_checksum_release_path_size_and_model_pin_tamper_are_refused(self):
        original = self.current_shape()
        for field, value in (("checksum", "sha256:" + "0" * 64), ("release_path", "model-catalog.json"), ("size", 0)):
            components = copy.deepcopy(original)
            components["external"]["localhost-provider"]["platforms"]["linux-amd64"][field] = value
            self.reapprove_components(components)
            with self.subTest(field=field), self.assertRaises(ValueError):
                self.prepare()
            for path in self.snapshot.iterdir():
                path.unlink()
        components = copy.deepcopy(original)
        components["model_catalog"]["head_cid"] = S.raw_cid(b"different catalog")
        self.reapprove_components(components)
        with self.assertRaisesRegex(ValueError, "catalog head differs"):
            self.prepare()

    def reapprove_components(self, components):
        data = S.json_bytes(components)
        (self.root / "components.json").write_bytes(data)
        record = {"cid": S.raw_cid(data), "sha256": S.sha256(data), "size": len(data)}
        self.manifest["files"]["components.json"] = record
        self.manifest["release"]["platforms"]["x86_64-linux"]["components"] = copy.deepcopy(record)
        self.approve_manifest()

    def test_single_chunk_unixfs_metadata_matches_existing_public_runtime_vector(self):
        data = b'{"payload":{"version":"0.7.1-rc.2"}}\n'
        expected = "QmZFMnZjkiTqy9VY5FDdKudvpsKtKxifk1poBrV7k4sqp8"
        self.assertEqual(S.unixfs_metadata_cid(data), expected)
        self.assertNotEqual(S.unixfs_metadata_cid(data[:-1]), expected)
        for value in (b"", b"x" * (256 * 1024 + 1)):
            with self.assertRaises(ValueError):
                S.unixfs_metadata_cid(value)

    def statement_policy(self):
        self.canary_policy()
        self.policy.update(signing_role="publisher-keys", max_statement_lifetime=1000,
                           max_future_skew=60, minimum_statement_version=2)
        self.manifest = {"source": {"commit": COMMIT, "tree": TREE}, "statement": {
            "schema": "elastos.publisher-keys/v1", "version": 2, "channel": "canary",
            "root_did": DID, "previous_root_did": None, "issued_at": 900,
            "expires_at": 1100, "release_dids": [S.public_did(bytes(32))]}}
        self.approve_manifest()

    def prepare_statement(self):
        return S.prepare(self.policy, self.root, "signing-input.json", self.fetch, self.snapshot, now=1000)

    def test_statement_mode_signs_only_approved_frozen_payload_in_its_domain(self):
        self.statement_policy()
        prepared = self.prepare_statement()
        approved = copy.deepcopy(self.manifest["statement"])
        self.manifest["statement"]["release_dids"] = [DID]
        self.approve_manifest()
        backend = FakeBackend()
        output = dict(S.sign_publication(prepared, backend))
        self.assertEqual(set(output), {"publisher-keys.json"})
        statement = S.parse_json(output["publisher-keys.json"])
        self.assertEqual(statement, {"payload": approved, "signatures": [
            {"signer_did": DID, "signature": bytes(64).hex()}]})
        expected = hashlib.sha256(b"elastos.publisher.keys.v1\0" + S.json_bytes(approved)).digest()
        self.assertEqual([call for call in backend.calls if isinstance(call, tuple) and call[0] == "sign"], [("sign", expected)])
        self.assertEqual(list(self.snapshot.iterdir()), [])
        self.assertFalse(self.marker.exists())

    def test_statement_publication_channel_uses_approved_develop_authority(self):
        self.statement_policy()
        for channel in ("canary", "stable", "jetson-test"):
            self.policy["channel"] = self.manifest["statement"]["channel"] = channel
            self.approve_manifest()
            self.prepare_statement()
            self.assertEqual(self.policy["channel"], channel)
        self.assertTrue(any("/git/ref/heads/develop" in path for path in self.requests))
        self.assertFalse(any("/git/ref/heads/main" in path or "/git/ref/tags/" in path or "/git/blobs/" in path for path in self.requests))
        self.policy["develop_oid"] = COMMIT
        with self.assertRaisesRegex(ValueError, "develop ref moved"):
            self.prepare_statement()

    def test_statement_revoke_all_delegates_emits_approved_empty_list(self):
        self.statement_policy()
        self.manifest["statement"]["release_dids"] = []
        self.approve_manifest()
        backend = FakeBackend()
        publication = dict(S.sign_publication(self.prepare_statement(), backend))
        statement = S.parse_json(publication["publisher-keys.json"])
        self.assertEqual(statement["payload"], self.manifest["statement"])
        self.assertEqual(statement["payload"]["release_dids"], [])
        self.assertEqual(statement["signatures"], [{"signer_did": DID, "signature": bytes(64).hex()}])
        self.assertEqual(len([call for call in backend.calls if isinstance(call, tuple) and call[0] == "sign"]), 1)

    def test_statement_mode_and_release_mode_refuse_each_others_inputs_before_backend(self):
        release = copy.deepcopy(self.manifest)
        self.statement_policy()
        statement = copy.deepcopy(self.manifest)
        for role, manifest in (("publisher-keys", release), ("release", statement), ("candidate-selected", statement)):
            self.policy["signing_role"] = role
            self.manifest = manifest
            self.approve_manifest()
            path = self.base / "operator-policy.json"
            path.write_bytes(S.json_bytes(self.policy))
            output = self.base / "publication"
            with self.subTest(role=role), \
                 mock.patch.object(sys, "argv", [str(SOURCE), "--policy", str(path), "--input-root", str(self.root), "--output-root", str(output)]), \
                 mock.patch.object(S, "pinned_tools"), mock.patch.object(S, "github_json", side_effect=self.fetch), \
                 mock.patch.object(S.time, "time", return_value=1000), mock.patch.object(S, "OpenSSLBackend") as backend:
                with self.assertRaises(ValueError):
                    S.main()
                backend.assert_not_called()
            self.assertFalse(output.exists())

    def test_statement_rejects_candidate_roles_domains_signatures_and_source_changes(self):
        self.statement_policy()
        approved = copy.deepcopy(self.manifest)
        changes = [{"signing_role": "release"}, {"domain": "elastos.release.v1"},
                   {"signatures": []}, {"source": {"commit": MAIN, "tree": TREE}},
                   {"source": {"commit": COMMIT, "tree": TREE, "extra": True}}]
        for change in changes:
            self.manifest = {**approved, **change}
            self.approve_manifest()
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.prepare_statement()
        self.manifest = approved
        for extra in ("signature", "signatures", "signer_did", "domain", "signing_role"):
            self.manifest = copy.deepcopy(approved)
            self.manifest["statement"][extra] = []
            self.approve_manifest()
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                self.prepare_statement()
        self.manifest = approved
        self.approve_manifest()
        (self.root / "signing-input.json").write_bytes(b"{}")
        with self.assertRaisesRegex(ValueError, "operator approval"):
            self.prepare_statement()
        data = S.json_bytes(approved).replace(b'"version":2', b'"version":2,"version":3')
        (self.root / "signing-input.json").write_bytes(data)
        self.policy["manifest_sha256"] = S.sha256(data)
        with self.assertRaisesRegex(ValueError, "duplicate JSON field"):
            self.prepare_statement()

    def test_statement_strict_payload_roots_and_delegate_bounds(self):
        self.statement_policy()
        approved = copy.deepcopy(self.manifest["statement"])
        other = S.public_did(bytes(32))
        changes = [{"schema": "elastos.release/v1"}, {"channel": "stable"}, {"channel": "unknown"},
                   {"root_did": "did:key:invalid"}, {"root_did": other},
                   {"previous_root_did": DID}, {"previous_root_did": "did:key:invalid"},
                   {"release_dids": [DID]}, {"release_dids": [other, other]},
                   {"release_dids": ["did:key:invalid"]}, {"release_dids": [other] * (S.MAX_RELEASE_DIDS + 1)},
                   {"release_dids": other}, {"previous_root_did": other, "release_dids": [other]}]
        for change in changes:
            self.manifest["statement"] = {**approved, **change}
            self.approve_manifest()
            with self.subTest(change=change), self.assertRaises(ValueError):
                self.prepare_statement()
        self.manifest["statement"] = {field: value for field, value in approved.items() if field != "previous_root_did"}
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "fields refused"):
            self.prepare_statement()

    def test_statement_nonobject_manifests_and_untyped_policy_channels_are_refused(self):
        self.statement_policy()
        for data in (b"[]", b"null", b"true", b'"statement"', b"1"):
            (self.root / "signing-input.json").write_bytes(data)
            self.policy["manifest_sha256"] = S.sha256(data)
            with self.subTest(data=data), self.assertRaisesRegex(ValueError, "JSON object required"):
                self.prepare_statement()
        self.approve_manifest()
        for channel in (True, False, None, [], {}, 1, "unknown"):
            self.policy["channel"] = channel
            with self.subTest(channel=channel), self.assertRaisesRegex(ValueError, "trusted statement channel"):
                self.prepare_statement()

    def test_statement_versions_times_and_explicit_policy_limits(self):
        self.statement_policy()
        approved = copy.deepcopy(self.manifest["statement"])
        for field, values in (("version", [0, 1, True, "2", 2**63]),
                              ("issued_at", [-1, True, "900", 1100, 1061]),
                              ("expires_at", [-1, True, "1100", 900, 1000, 1901])):
            for value in values:
                self.manifest["statement"] = {**approved, field: value}
                self.approve_manifest()
                with self.subTest(field=field, value=value), self.assertRaises(ValueError):
                    self.prepare_statement()
        self.manifest["statement"] = {**approved, "issued_at": 1060}
        self.approve_manifest()
        self.prepare_statement()
        self.manifest["statement"] = {**approved, "expires_at": 1001}
        self.approve_manifest()
        self.prepare_statement()
        self.manifest["statement"] = approved
        self.approve_manifest()
        original_policy = copy.deepcopy(self.policy)
        for field in ("max_statement_lifetime", "max_future_skew", "minimum_statement_version"):
            for value in (None, True, "1", -1, 2**63):
                self.policy = {**original_policy, field: value}
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, "explicit statement"):
                    self.prepare_statement()
            self.policy = copy.deepcopy(original_policy)
            del self.policy[field]
            with self.assertRaisesRegex(ValueError, "explicit statement"):
                self.prepare_statement()
        self.policy = {**original_policy, "max_future_skew": 0}
        self.prepare_statement()
        for now in (True, -1, 2**63):
            with self.subTest(now=now), self.assertRaisesRegex(ValueError, "admission time"):
                S.prepare(self.policy, self.root, "signing-input.json", self.fetch, self.snapshot, now=now)

    def test_rotation_output_has_only_the_explicitly_approved_roots_one_signature(self):
        self.statement_policy()
        previous = S.public_did(bytes(32))
        delegate = S.public_did(bytes(reversed(PUBLIC)))
        self.manifest["statement"].update(previous_root_did=previous, release_dids=[delegate])
        self.approve_manifest()
        for signer, public in ((DID, PUBLIC), (previous, bytes(32))):
            self.policy["publisher_did"] = signer
            prepared = self.prepare_statement()
            backend = FakeBackend()
            backend.public = public
            output = S.parse_json(dict(S.sign_publication(prepared, backend))["publisher-keys.json"])
            self.assertEqual(output["signatures"], [{"signer_did": signer, "signature": bytes(64).hex()}])
        self.policy["publisher_did"] = delegate
        with self.assertRaisesRegex(ValueError, "outside statement roots"):
            self.prepare_statement()

    def test_statement_backend_refusals_and_cancellation(self):
        self.statement_policy()
        prepared = self.prepare_statement()
        for attribute, value, reason in (("public", bytes(32), "DID"), ("signature", bytes(63), "length"), ("verified", False, "verification")):
            backend = FakeBackend()
            setattr(backend, attribute, value)
            with self.subTest(attribute=attribute), self.assertRaisesRegex(ValueError, reason):
                S.sign_publication(prepared, backend)
        prompt = io.StringIO()
        self.assertFalse(S.confirmed(prepared, io.StringIO("\n"), prompt))
        self.assertIn("one root signature", prompt.getvalue())
        self.assertTrue(S.confirmed(prepared, io.StringIO(DID + "\n"), io.StringIO()))

    def test_statement_cli_uses_one_admission_time_rechecks_develop_and_writes_only_statement(self):
        self.statement_policy()
        self.policy["channel"] = self.manifest["statement"]["channel"] = "stable"
        self.approve_manifest()
        path = self.base / "operator-policy.json"
        path.write_bytes(S.json_bytes(self.policy))
        output = self.base / "publication"
        backend = FakeBackend()
        with mock.patch.object(sys, "argv", [str(SOURCE), "--policy", str(path), "--input-root", str(self.root), "--output-root", str(output)]), \
             mock.patch.object(S, "pinned_tools"), mock.patch.object(S, "github_json", side_effect=self.fetch), \
             mock.patch.object(S.time, "time", return_value=1000) as clock, \
             mock.patch.object(sys, "stdin", io.StringIO(DID + "\n")), \
             mock.patch.object(sys, "stdout", io.StringIO()), mock.patch.object(sys, "stderr", io.StringIO()), \
             mock.patch.object(S, "OpenSSLBackend", return_value=backend) as factory:
            S.main()
        clock.assert_called_once_with()
        factory.assert_called_once()
        self.assertEqual(sum(path.endswith("/git/ref/heads/develop") for path in self.requests), 2)
        self.assertEqual([path.name for path in output.iterdir()], ["publisher-keys.json"])
        self.assertEqual((output / "publisher-keys.json").stat().st_mode & 0o777, 0o444)
        self.assertEqual(S.parse_json((output / "publisher-keys.json").read_bytes())["payload"], self.manifest["statement"])
        self.assertIn("close", backend.calls)
        self.assertFalse(any(path.name.startswith(".elastos-signing-") for path in self.base.iterdir()))

    def test_statement_input_uses_held_descriptor_and_refuses_links_and_disk_floor(self):
        self.statement_policy()
        self.root.rename(self.base / "held-candidate")
        self.root.mkdir()
        # Descriptor holding is exercised directly with the original directory.
        held = S.directory_fd(self.base / "held-candidate")
        try:
            prepared = S.prepare_statement(self.policy, S.signing_source_policy(self.policy), self.root,
                                          "signing-input.json", self.fetch, self.snapshot, held, 1000)
            self.assertEqual(S.parse_json(prepared.statement), self.manifest["statement"])
        finally:
            os.close(held)
        source = self.base / "held-candidate/signing-input.json"
        candidate = self.root / "signing-input.json"
        candidate.symlink_to(source)
        with self.assertRaises(OSError):
            self.prepare_statement()
        candidate.unlink()
        os.link(source, candidate)
        with self.assertRaisesRegex(ValueError, "regular unlinked file"):
            self.prepare_statement()
        candidate.unlink()
        candidate.write_bytes(source.read_bytes())
        with mock.patch.object(S.shutil, "disk_usage", return_value=SimpleNamespace(total=100, free=14)), \
             self.assertRaisesRegex(ValueError, "free-space floor"):
            self.prepare_statement()
        for quota in ("max_file_bytes", "max_snapshot_bytes"):
            original = self.policy[quota]
            self.policy[quota] = 1
            with self.subTest(quota=quota), self.assertRaisesRegex(ValueError, "snapshot quota"):
                self.prepare_statement()
            self.policy[quota] = original


if __name__ == "__main__":
    unittest.main()
