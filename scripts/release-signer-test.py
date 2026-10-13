#!/usr/bin/env python3
"""Public-data/fake-backend refusal tests. One model catalogue round trip signs
with a throwaway OpenSSL key; no other private keys or real signing."""

import base64
import copy
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
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
WORKFLOW, WORKFLOW_TREE = "9" * 40, "8" * 40
DEVELOP_TREE, WORKFLOW_BLOB = "f" * 40, "7" * 40


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
        (self.root / "elastos-x86_64-linux").write_bytes(self.binary)
        (self.root / "elastos-x86_64-linux").chmod(0o755)
        self.source_components = S.json_bytes({"external": {"localhost-provider": {}}, "profiles": {}})
        self.source_components_oid = hashlib.sha1(b"blob " + str(len(self.source_components)).encode() + b"\0" + self.source_components).hexdigest()
        self.platform_components, self.unattested = {}, set()
        self.template = ("#!/bin/sh\n" + "\n".join(f'{name}="__{name}__"' for name in sorted(S.STAMPS | {"HEAD_CID"})) + f"\ncat /custodian/unopened-signing.pem > {self.marker}\n").encode()
        self.blob_oid = hashlib.sha1(b"blob " + str(len(self.template)).encode() + b"\0" + self.template).hexdigest()
        node = "4" * 64
        ticket = base64.b32encode(S.json_bytes({"endpoints": [{"id": node, "addrs": []}]})).decode().lower().rstrip("=")
        self.stamps = {"MAINTAINER_DID": DID, "SOURCE_CONNECT_TICKET": ticket,
                       "PUBLISHER_GATEWAY": "https://publisher.invalid", "PUBLISHER_NODE_ID": node, "IPNS_NAME": "public-ipns"}
        rendered = S.render_installer(self.template, self.stamps, DID)
        def record(data):
            return {"sha256": S.sha256(data), "size": len(data), "cid": S.raw_cid(data)}
        self.manifest = {"source": {"commit": COMMIT, "tree": TREE}, "version": "1.2.3", "channel": "stable",
                         "files": {"elastos-x86_64-linux": record(self.binary)},
                         "installer": {"blob_oid": self.blob_oid, "stamps": self.stamps},
                         "release": {"schema": "elastos.release/v1", "source": {"commit": COMMIT, "tree": TREE},
                                     "version": "1.2.3", "channel": "stable", "released_at": 1, "prev_release_cid": None,
                                     "platforms": {"x86_64-linux": {"binary": {k: record(self.binary)[k] for k in ("cid", "sha256")}}},
                                     "installer_sha256": S.sha256(rendered)},
                         "head": {"updated_at": 2, "prev_head_cid": None}}
        self.policy = {"repository": S.REPOSITORY, "tag": "v1.2.3", "tag_oid": COMMIT, "commit": COMMIT,
                       "tree": TREE, "version": "1.2.3", "channel": "stable", "publisher_did": DID,
                       "max_file_bytes": 1024 * 1024, "max_snapshot_bytes": 4 * 1024 * 1024,
                       "key_path": "/custodian/unopened-signing.pem"}
        self.approval = {"schema": S.APPROVAL_SCHEMA, "repository": S.REPOSITORY,
                         "source": {"commit": COMMIT, "tree": TREE},
                         "workflow": {"commit": WORKFLOW, "tree": WORKFLOW_TREE, "ref": "refs/heads/develop"},
                         "delivery": self.stamps,
                         "gh": {"path": str(SOURCE), "sha256": S.sha256(SOURCE.read_bytes())}}
        self.approval_path = self.base / "approval.json"
        self.approval_path.write_bytes(S.json_bytes(self.approval))
        self.approval_path.chmod(0o600)
        self.policy["approval"] = {"path": str(self.approval_path), "sha256": S.sha256(self.approval_path.read_bytes())}
        self.package_run = {"id": 123, "run_attempt": 2, "head_sha": WORKFLOW, "head_branch": "develop",
                            "path": S.PACKAGE_WORKFLOW, "event": "workflow_dispatch", "status": "completed", "conclusion": "success",
                            "head_repository": {"id": 1234, "full_name": S.REPOSITORY},
                            "repository": {"id": 1234, "full_name": S.REPOSITORY}}
        self.approve_manifest()
        prefix = f"/repos/{S.REPOSITORY}"
        self.api = {
            f"{prefix}/git/ref/tags/v1.2.3": {"ref": "refs/tags/v1.2.3", "object": {"type": "commit", "sha": COMMIT}},
            f"{prefix}/git/commits/{COMMIT}": {"sha": COMMIT, "tree": {"sha": TREE}},
            f"{prefix}/git/commits/{WORKFLOW}": {"sha": WORKFLOW, "tree": {"sha": WORKFLOW_TREE}},
            f"{prefix}/actions/runs/123": self.package_run,
            f"{prefix}/git/ref/heads/main": {"ref": "refs/heads/main", "object": {"type": "commit", "sha": MAIN}},
            f"{prefix}/git/commits/{MAIN}": {"sha": MAIN, "tree": {"sha": DEVELOP_TREE}},
            f"{prefix}/git/ref/heads/develop": {"ref": "refs/heads/develop", "object": {"type": "commit", "sha": DEVELOP}},
            f"{prefix}/git/commits/{DEVELOP}": {"sha": DEVELOP, "tree": {"sha": DEVELOP_TREE}},
            f"{prefix}/compare/{COMMIT}...{MAIN}?per_page=1": {"status": "ahead", "behind_by": 0, "ahead_by": 1,
                "base_commit": {"sha": COMMIT}, "merge_base_commit": {"sha": COMMIT}},
            f"{prefix}/git/trees/{TREE}": {"sha": TREE, "truncated": False, "tree": [{"path": "scripts", "type": "tree", "mode": "040000", "sha": SCRIPTS},
                {"path": "components.json", "type": "blob", "mode": "100644", "sha": self.source_components_oid}]},
            f"{prefix}/git/trees/{SCRIPTS}": {"sha": SCRIPTS, "truncated": False, "tree": [{"path": "install.sh", "type": "blob", "mode": "100755", "sha": self.blob_oid}]},
            f"{prefix}/git/blobs/{self.blob_oid}": {"sha": self.blob_oid, "encoding": "base64", "size": len(self.template), "content": base64.b64encode(self.template).decode()},
            f"{prefix}/git/blobs/{self.source_components_oid}": {"sha": self.source_components_oid, "encoding": "base64",
                "size": len(self.source_components), "content": base64.b64encode(self.source_components).decode()},
        }
        for tree in (TREE, WORKFLOW_TREE, DEVELOP_TREE):
            self.workflow_blob(tree, WORKFLOW_BLOB)
        self.requests = []
        self.no_process = mock.patch.object(S.subprocess, "run", side_effect=AssertionError("real subprocess refused in tests"))
        self.no_process.start()
        self.addCleanup(self.no_process.stop)
        self.proof = mock.patch.object(S, "attestation_result", side_effect=self.verified_receipt)
        self.proof.start()
        self.addCleanup(self.proof.stop)
        # Unit fixtures use a deterministic healthy volume. The dedicated quota
        # case below overrides this with a full volume to prove the real guard.
        disk = mock.patch.object(S.shutil, "disk_usage", return_value=SimpleNamespace(
            total=100 * 1024**3, free=50 * 1024**3))
        disk.start()
        self.addCleanup(disk.stop)

    def workflow_blob(self, tree, blob):
        prefix = f"/repos/{S.REPOSITORY}/git/trees/"
        github, workflows = (hashlib.sha1((tree + part).encode()).hexdigest() for part in (".github", "workflows"))
        listing = self.api.setdefault(prefix + tree, {"sha": tree, "truncated": False, "tree": []})["tree"]
        listing[:] = [entry for entry in listing if entry["path"] != ".github"]
        listing.append({"path": ".github", "type": "tree", "mode": "040000", "sha": github})
        self.api[prefix + github] = {"sha": github, "truncated": False,
                                     "tree": [{"path": "workflows", "type": "tree", "mode": "040000", "sha": workflows}]}
        self.api[prefix + workflows] = {"sha": workflows, "truncated": False,
                                        "tree": [{"path": "release-package.yml", "type": "blob", "mode": "100644", "sha": blob}]}

    def approve_manifest(self):
        directory = self.root / "provenance"
        directory.mkdir(exist_ok=True)
        platforms = list(self.manifest["release"]["platforms"])
        attested = {platform: S.json_bytes(self.platform_components.get(platform, {"schema": "elastos.components/v1", "external": {}}))
                    for platform in platforms}
        cids = {name: record["cid"] for name, record in self.manifest["files"].items()}
        generated = S.json_bytes(S.bind_component_cids(S.merge_components(
            S.parse_json(self.source_components), {p: S.parse_json(data) for p, data in attested.items()}), cids))
        for platform in platforms:
            name = f"components-{platform}.json"
            (self.root / name).write_bytes(generated)
            self.manifest["files"][name] = {"sha256": S.sha256(generated), "size": len(generated), "cid": S.raw_cid(generated)}
            self.manifest["release"]["platforms"][platform]["components"] = {
                key: self.manifest["files"][name][key] for key in ("cid", "sha256", "size")}
        entries = []
        for index, platform in enumerate(platforms):
            receipt = {"schema": "elastos.release-platform-input/v1", "platform": platform, "version": self.manifest["version"],
                       "source": {**self.manifest["source"], "clean": True},
                       "files": {"artifacts/" + name: {"sha256": record["sha256"], "size": record["size"], "executable": True}
                                 for name, record in self.manifest["files"].items()
                                 if not name.startswith("components-") and name not in self.unattested}}
            native = self.manifest["files"].get("elastos-" + platform, self.manifest["files"]["elastos-x86_64-linux"])
            receipt["files"]["artifacts/elastos-" + platform] = {"sha256": native["sha256"], "size": native["size"], "executable": True}
            for name, data in (("components.json", attested[platform]), ("components-template.json", self.source_components)):
                receipt["files"][name] = {"sha256": S.sha256(data), "size": len(data), "executable": False}
            receipt_name = f"provenance/{platform}.receipt.json"
            bundle_name = f"provenance/{platform}.bundle.jsonl"
            components_name = f"provenance/{platform}.components.json"
            prefix = "release-mac" if platform == "aarch64-darwin" else "release-" + platform
            artifact = {"id": index + 1, "digest": "sha256:" + "6" * 64, "name": f"{prefix}-{COMMIT}-2", "attempt": 2}
            proof_artifact = {"id": index + 101, "digest": "sha256:" + "7" * 64,
                              "name": f"{prefix.replace('release-', 'release-proof-', 1)}-{COMMIT}-2", "attempt": 2}
            receipt["producer"] = {"artifact": artifact, "run_id": 123, "workflow_commit": WORKFLOW, "attestation_attempt": 2}
            (self.root / receipt_name).write_bytes(S.json_bytes(receipt))
            (self.root / bundle_name).write_bytes(b"{}")
            (self.root / components_name).write_bytes(attested[platform])
            entries.append({"platform": platform, "receipt": receipt_name, "bundle": bundle_name, "components": components_name,
                            "artifact": artifact, "proof_artifact": proof_artifact})
        self.manifest["provenance"] = {"schema": S.PROVENANCE_SCHEMA, "run_id": 123, "run_attempt": 2,
                                       "approval_sha256": self.policy["approval"]["sha256"], "receipts": entries}
        data = S.json_bytes(self.manifest)
        (self.root / "signing-input.json").write_bytes(data)
        self.policy["manifest_sha256"] = S.sha256(data)

    def fetch(self, path):
        self.requests.append(path)
        if path == f"/repos/{S.REPOSITORY}/actions/runs/123/artifacts?per_page=100&page=1":
            return {"artifacts": [{**entry[kind], "expired": False,
                                    "workflow_run": {"id": 123, "head_sha": WORKFLOW, "head_branch": "develop"}}
                                   for entry in self.manifest["provenance"]["receipts"] for kind in ("artifact", "proof_artifact")]}
        if path not in self.api:
            raise ValueError("remote object unavailable")
        return copy.deepcopy(self.api[path])

    def verified_receipt(self, approval, run, receipt_path, bundle_path, scratch):
        workflow = approval["workflow"]
        uri = f"https://github.com/{S.REPOSITORY}/{S.PACKAGE_WORKFLOW}@{workflow['ref']}"
        certificate = {"issuer": "https://token.actions.githubusercontent.com", "subjectAlternativeName": uri,
                       "buildSignerURI": uri, "buildSignerDigest": workflow["commit"], "runnerEnvironment": "github-hosted",
                       "sourceRepositoryURI": f"https://github.com/{S.REPOSITORY}", "sourceRepositoryDigest": workflow["commit"],
                       "sourceRepositoryIdentifier": "1234", "sourceRepositoryRef": workflow["ref"],
                       "buildConfigURI": uri, "buildConfigDigest": workflow["commit"], "buildTrigger": "workflow_dispatch",
                       "runInvocationURI": f"https://github.com/{S.REPOSITORY}/actions/runs/{run['id']}/attempts/{run['run_attempt']}"}
        return [{"verificationResult": {"signature": {"certificate": certificate}, "verifiedTimestamps": [{"type": "Tlog"}],
                                          "statement": {"predicateType": "https://slsa.dev/provenance/v1",
                                                        "subject": [{"digest": {"sha256": S.sha256(receipt_path.read_bytes())}}]}}}]

    def prepare(self):
        return S.prepare(self.policy, self.root, "signing-input.json", self.fetch, self.snapshot)

    def test_snapshot_signs_only_frozen_bytes_and_binds_installer_release_head(self):
        prepared = self.prepare()
        (self.root / "elastos-x86_64-linux").write_bytes(b"changed after snapshot")
        (self.root / "components-x86_64-linux.json").write_bytes(b"changed after snapshot")
        backend = FakeBackend()
        publication = dict(S.sign_publication(prepared, backend))
        self.assertEqual(publication["elastos-x86_64-linux"].read_bytes(), self.binary)
        release = S.parse_json(publication["release.json"])
        head = S.parse_json(publication["release-head.json"])
        self.assertEqual(release["payload"]["installer_sha256"], S.sha256(publication["install.sh"].read_bytes()))
        self.assertEqual(head["payload"]["release_sha256"], S.sha256(publication["release.json"]))
        self.assertEqual(head["payload"]["latest_release_cid"], S.unixfs_metadata_cid(publication["release.json"]))
        self.assertIn(b'HEAD_CID=""', publication["install.sh"].read_bytes())
        self.assertFalse(self.marker.exists())
        self.assertEqual(len([call for call in backend.calls if isinstance(call, tuple) and call[0] == "sign"]), 2)

    def test_source_and_approved_builder_are_independent(self):
        self.assertNotEqual(self.policy["commit"], self.approval["workflow"]["commit"])
        self.prepare()
        self.assertIn(f"/repos/{S.REPOSITORY}/git/commits/{WORKFLOW}", self.requests)

    def test_retained_native_and_proof_attempts_survive_a_later_successful_retry(self):
        self.package_run['run_attempt'] = 3
        self.manifest['provenance']['run_attempt'] = 3
        entry = self.manifest['provenance']['receipts'][0]
        entry['artifact']['name'] = entry['artifact']['name'].rsplit('-', 1)[0] + '-1'
        entry['artifact']['attempt'] = 1
        receipt_path = self.root / entry['receipt']
        receipt = S.parse_json(receipt_path.read_bytes())
        receipt['producer']['artifact'] = entry['artifact']
        receipt_path.write_bytes(S.json_bytes(receipt))
        data = S.json_bytes(self.manifest)
        (self.root / 'signing-input.json').write_bytes(data)
        self.policy['manifest_sha256'] = S.sha256(data)
        self.prepare()

    def test_receipt_artifact_attempt_and_attestation_attempt_are_independent_exact_bindings(self):
        entry = self.manifest['provenance']['receipts'][0]
        path = self.root / entry['receipt']
        original = S.parse_json(path.read_bytes())
        for field in ('artifact', 'attestation_attempt'):
            receipt = copy.deepcopy(original)
            if field == 'artifact':
                receipt['producer'][field]['attempt'] = 1
            else:
                receipt['producer'][field] = 1
            path.write_bytes(S.json_bytes(receipt))
            with self.subTest(field=field), self.assertRaisesRegex(ValueError, 'attested build artifact differs'):
                self.prepare()
        path.write_bytes(S.json_bytes(original))

    def test_foreign_modified_incomplete_and_wrong_event_producers_are_refused(self):
        for field, value in (("head_sha", COMMIT), ("head_branch", "feature/edited-workflow"),
                             ("event", "pull_request_target"), ("status", "in_progress"),
                             ("conclusion", "failure"), ("path", ".github/workflows/other.yml"),
                             ("head_repository", {"id": 999, "full_name": "attacker/fork"}),
                             ("run_attempt", True)):
            with self.subTest(field=field), mock.patch.dict(self.package_run, {field: value}):
                with self.assertRaisesRegex(ValueError, "producer|attempt"):
                    self.prepare()
            for path in self.snapshot.iterdir():
                path.unlink()

    def test_workflow_tree_and_source_tree_are_checked_independently(self):
        self.api[f"/repos/{S.REPOSITORY}/git/commits/{WORKFLOW}"]["tree"]["sha"] = TREE
        with self.assertRaisesRegex(ValueError, "producer/source tree"):
            self.prepare()

    def test_package_workflow_must_match_develop(self):
        self.workflow_blob(WORKFLOW_TREE, "6" * 40)
        with self.assertRaisesRegex(ValueError, "package workflow differs from develop"):
            self.prepare()
        self.workflow_blob(WORKFLOW_TREE, WORKFLOW_BLOB)
        self.workflow_blob(DEVELOP_TREE, "6" * 40)
        with self.assertRaisesRegex(ValueError, "package workflow differs from develop"):
            S.checked_package_run(self.approval, 123, self.fetch)

    def test_only_develop_producers_are_approved(self):
        self.approval["workflow"]["ref"] = "refs/heads/main"
        self.package_run["head_branch"] = "main"
        self.approval_path.write_bytes(S.json_bytes(self.approval))
        with self.assertRaisesRegex(ValueError, "approved develop producer ref required"):
            S.approved_release(self.approval_path)
        with self.assertRaisesRegex(ValueError, "package run differs"):
            S.checked_package_run({**self.approval, "workflow": {**self.approval["workflow"], "ref": "refs/heads/develop"}}, 123, self.fetch)

    def test_independent_approval_and_tool_pins_are_required(self):
        pin = self.policy.pop("approval")
        with self.assertRaisesRegex(ValueError, "approval pin"):
            self.prepare()
        self.policy["approval"] = pin
        self.approval_path.write_bytes(self.approval_path.read_bytes() + b" ")
        with self.assertRaisesRegex(ValueError, "approval changed"):
            self.prepare()

    def test_candidate_owned_approval_is_refused_even_with_a_matching_hash(self):
        candidate = self.root / "approval.json"
        candidate.write_bytes(self.approval_path.read_bytes())
        self.policy["approval"]["path"] = str(candidate)
        with self.assertRaisesRegex(ValueError, "outside candidate"):
            self.prepare()

    def test_candidate_manifest_cannot_reapprove_a_delivery_change(self):
        self.manifest["installer"]["stamps"] = {**self.stamps, "PUBLISHER_NODE_ID": "5" * 64}
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "delivery differs"):
            self.prepare()

    def test_approved_ticket_must_bind_every_endpoint_to_the_approved_node(self):
        for ids in (["5" * 64], ["4" * 64, "5" * 64], []):
            stamps = {**self.stamps, "SOURCE_CONNECT_TICKET": base64.b32encode(S.json_bytes({
                "endpoints": [{"id": node, "addrs": []} for node in ids]})).decode().lower().rstrip("=")}
            with self.subTest(ids=ids), self.assertRaisesRegex(ValueError, "every approved ticket endpoint"):
                S.validate_delivery(stamps)
        S.validate_delivery(self.stamps)

    def test_immutable_attestation_certificate_and_subject_are_required(self):
        for field, value in (("issuer", "https://issuer.invalid"),
                             ("subjectAlternativeName", f"https://github.com/{S.REPOSITORY}/.github/workflows/foreign.yml@refs/heads/develop"),
                             ("buildSignerURI", f"https://github.com/{S.REPOSITORY}/{S.PACKAGE_WORKFLOW}@refs/heads/foreign"),
                             ("buildConfigURI", f"https://github.com/{S.REPOSITORY}/.github/workflows/foreign.yml@refs/heads/develop"),
                             ("buildTrigger", "pull_request"), ("runnerEnvironment", "self-hosted"),
                             ("sourceRepositoryDigest", COMMIT), ("buildSignerDigest", COMMIT),
                             ("sourceRepositoryRef", "refs/heads/feature"), ("sourceRepositoryIdentifier", "999"),
                             ("buildConfigDigest", COMMIT),
                             ("runInvocationURI", f"https://github.com/{S.REPOSITORY}/actions/runs/123/attempts/1")):
            def altered(*args):
                result = self.verified_receipt(*args)
                result[0]["verificationResult"]["signature"]["certificate"][field] = value
                # Candidate-controlled predicate values cannot repair a certificate.
                result[0]["verificationResult"]["statement"]["predicate"] = {"run_id": 123, "commit": WORKFLOW}
                return result
            with self.subTest(field=field), mock.patch.object(S, "attestation_result", side_effect=altered):
                with self.assertRaisesRegex(ValueError, "certificate"):
                    self.prepare()
            for path in self.snapshot.iterdir():
                path.unlink()
        with mock.patch.object(S, "attestation_result", return_value=[]):
            with self.assertRaisesRegex(ValueError, "verified build attestation"):
                self.prepare()

    def test_forged_native_bytes_cannot_be_approved_by_only_the_manifest_hash(self):
        malicious = b"x" * len(self.binary)
        (self.root / "elastos-x86_64-linux").write_bytes(malicious)
        record = self.manifest["files"]["elastos-x86_64-linux"]
        record.update(sha256=S.sha256(malicious), cid=S.raw_cid(malicious))
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"] = {k: record[k] for k in ("sha256", "cid")}
        data = S.json_bytes(self.manifest)
        (self.root / "signing-input.json").write_bytes(data)
        self.policy["manifest_sha256"] = S.sha256(data)
        with self.assertRaisesRegex(ValueError, "lack approved producer proof"):
            self.prepare()

    def test_wrong_receipt_subject_digest_is_refused(self):
        def altered(*args):
            result = self.verified_receipt(*args)
            result[0]["verificationResult"]["statement"]["subject"][0]["digest"]["sha256"] = "0" * 64
            return result
        with mock.patch.object(S, "attestation_result", side_effect=altered):
            with self.assertRaisesRegex(ValueError, "certificate"):
                self.prepare()

    def test_verifier_uses_exact_identity_ref_digests_and_isolated_environment(self):
        self.proof.stop()
        receipt = self.root / "provenance/x86_64-linux.receipt.json"
        bundle = self.root / "provenance/x86_64-linux.bundle.jsonl"
        result = self.verified_receipt(self.approval, self.package_run, receipt, bundle, self.snapshot)
        with mock.patch.object(S.subprocess, "run", return_value=SimpleNamespace(returncode=0, stdout=S.json_bytes(result)) ) as process:
            actual = S.attestation_result(self.approval, self.package_run, receipt, bundle, self.snapshot)
        self.assertEqual(actual, result)
        command = process.call_args.args[0]
        self.assertEqual(command[0], self.approval["gh"]["path"])
        self.assertIn("--deny-self-hosted-runners", command)
        identities = {"--cert-identity", "--cert-identity-regex", "--signer-repo", "--signer-workflow"}
        self.assertEqual(identities.intersection(command), {"--cert-identity"})
        for flag, expected in (("--source-digest", WORKFLOW), ("--signer-digest", WORKFLOW),
                               ("--source-ref", "refs/heads/develop"),
                               ("--cert-identity", f"https://github.com/{S.REPOSITORY}/{S.PACKAGE_WORKFLOW}@refs/heads/develop")):
            self.assertEqual(command[command.index(flag) + 1], expected)
        self.assertEqual(set(process.call_args.kwargs["env"]), {"PATH", "HOME", "GH_CONFIG_DIR", "GH_HOST", "GH_PROMPT_DISABLED", "GH_NO_UPDATE_NOTIFIER"})

    def test_availability_failures_ask_for_retry_and_differ_from_provenance_refusals(self):
        path = f"/repos/{S.REPOSITORY}/git/commits/{COMMIT}"
        def response(status, **headers):
            reply = mock.Mock(status=status)
            reply.getheader.side_effect = lambda name, default=None: headers.get(name.replace("-", "_"), default)
            return reply
        for outcome, retry in ((OSError("Network is unreachable"), True), (response(429), True), (response(503), True),
                               (response(403, X_RateLimit_Remaining="0"), True), (response(404), False), (response(403), False)):
            connection = mock.Mock()
            if isinstance(outcome, Exception):
                connection.request.side_effect = outcome
            else:
                connection.getresponse.return_value = outcome
            with self.subTest(outcome=outcome), mock.patch.object(S.http.client, "HTTPSConnection", return_value=connection), \
                    mock.patch.dict(S.os.environ, {"GH_TOKEN": "operator-token"}):
                with self.assertRaisesRegex(ValueError, "retry later. This is not a provenance refusal" if retry
                                            else "GitHub source verification failed"):
                    S.github_json(path)
                self.assertEqual(connection.request.call_args.kwargs["headers"]["Authorization"], "Bearer operator-token")
        receipt = self.root / "provenance/x86_64-linux.receipt.json"
        bundle = self.root / "provenance/x86_64-linux.bundle.jsonl"
        for outcome, expected in ((subprocess.TimeoutExpired("gh", 120), S.UNAVAILABLE),
                                  (SimpleNamespace(returncode=1, stdout=b"", stderr=b"dial tcp: lookup tuf-repo-cdn.sigstore.dev: no such host"),
                                   S.UNAVAILABLE),
                                  (SimpleNamespace(returncode=1, stdout=b"",
                                                   stderr=b"error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com"),
                                   S.UNAVAILABLE),
                                  (SimpleNamespace(returncode=1, stdout=b"", stderr=b"unsupported media type"),
                                   "approved build attestation verification failed")):
            self.proof.stop()
            with self.subTest(outcome=outcome), mock.patch.object(S.subprocess, "run", side_effect=[outcome] if isinstance(outcome, Exception) else None,
                                                                  return_value=outcome):
                with self.assertRaises(ValueError) as caught:
                    S.attestation_result(self.approval, self.package_run, receipt, bundle, self.snapshot)
                self.assertIn(expected, str(caught.exception))
            self.proof.start()
        provenance_spec = importlib.util.spec_from_file_location("release_provenance", SOURCE.with_name("release-provenance.py"))
        provenance = importlib.util.module_from_spec(provenance_spec)
        provenance_spec.loader.exec_module(provenance)
        for stderr, retry in ((b"error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com", True),
                              (b"gh: Server Error (HTTP 502)", True), (b"gh: API rate limit exceeded (HTTP 403)", True),
                              (b"", True), (b"gh: Not Found (HTTP 404)", False)):
            with self.subTest(stderr=stderr), mock.patch.object(provenance.subprocess, "run",
                                                                return_value=SimpleNamespace(returncode=1, stdout=b"", stderr=stderr)):
                with self.assertRaises(ValueError) as caught:
                    provenance.cli_fetch(self.approval, f"/repos/{S.REPOSITORY}/actions/runs/123")
                self.assertEqual("not a provenance refusal" in str(caught.exception), retry)

    def test_installed_gh_accepts_production_flags_and_refuses_the_inert_bundle(self):
        # Exercise the real CLI parser with an empty test-only trust-root set.
        # This keeps parser proof offline and refuses the inert bundle. The
        # exact production vector was also checked with installed gh 2.101.0;
        # successful hosted attestation remains a separate integration gate.
        executable = shutil.which("gh")
        if executable is None:
            self.skipTest("installed gh is required for CLI compatibility proof")
        executable = Path(executable).resolve()
        self.assertFalse(executable.is_relative_to(self.root))
        self.proof.stop()
        self.no_process.stop()
        real_run = subprocess.run
        receipt = self.root / "provenance/x86_64-linux.receipt.json"
        bundle = self.root / "provenance/x86_64-linux.bundle.jsonl"
        self.assertEqual(bundle.read_bytes(), b"{}")
        approval = copy.deepcopy(self.approval)
        approval["gh"] = {"path": str(executable), "sha256": S.sha256(executable.read_bytes())}
        calls = []
        roots = self.base / "empty-test-trusted-roots.jsonl"
        roots.write_bytes(b"")
        offline = ["--custom-trusted-root", str(roots)]

        def execute(*args, **kwargs):
            result = real_run(args[0] + offline, **kwargs)
            calls.append((args[0], kwargs, result))
            return result

        with mock.patch.object(S.subprocess, "run", side_effect=execute):
            with self.assertRaisesRegex(ValueError, "attestation verification failed"):
                S.attestation_result(approval, self.package_run, receipt, bundle, self.snapshot)
        command, options, result = calls[0]
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(b"unsupported media type", result.stderr)
        self.assertNotIn(b"were all set", result.stderr)
        self.assertNotIn(b"unknown flag", result.stderr)
        self.assertEqual(options["env"]["GH_CONFIG_DIR"], str(self.snapshot / "gh"))
        # Reproduce the rejected former command with exactly the same inert
        # payload, bundle and credential-free environment.
        old = real_run(command + ["--signer-workflow", f"{S.REPOSITORY}/{S.PACKAGE_WORKFLOW}"] + offline,
                       **{**options, "timeout": 10})
        self.assertNotEqual(old.returncode, 0)
        self.assertIn(b"[cert-identity signer-workflow] were all set", old.stderr)

    def test_bounded_changes_are_preserved_in_signed_release(self):
        for changes in ([], ["é" * 250], ["change"] * 32,
                        ["x" * 500] * 16 + ["x" * 192]):
            with self.subTest(changes=changes):
                self.manifest["release"]["changes"] = changes
                self.approve_manifest()
                publication = dict(S.sign_publication(self.prepare(), FakeBackend()))
                release = S.parse_json(publication["release.json"])
                self.assertEqual(release["payload"]["changes"], changes)
                for path in self.snapshot.iterdir():
                    path.unlink()

    def test_changes_outside_runtime_bounds_are_refused(self):
        for changes in (None, "change", {}, [1], [True], [None], [""], ["   "],
                        ["x" * 501], ["é" * 251], ["change"] * 33,
                        ["x" * 500] * 16 + ["x" * 193],
                        *(["before" + chr(code) + "after"] for code in (0, 9, 10, 31, 127, 159))):
            with self.subTest(changes=changes):
                self.manifest["release"]["changes"] = changes
                self.approve_manifest()
                with self.assertRaisesRegex(ValueError, "changes"):
                    self.prepare()
                for path in self.snapshot.iterdir():
                    path.unlink()

    def test_changes_keep_other_release_fields_strict(self):
        self.manifest["release"]["changes"] = ["A change"]
        self.manifest["release"]["extra"] = "refuse"
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "release fields refused"):
            self.prepare()
        for path in self.snapshot.iterdir():
            path.unlink()
        del self.manifest["release"]["extra"]
        del self.manifest["release"]["installer_sha256"]
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "release fields refused"):
            self.prepare()

    def test_signer_accepts_all_three_release_platforms(self):
        platform = self.manifest["release"]["platforms"]["x86_64-linux"]
        self.manifest["release"]["platforms"] = {
            name: copy.deepcopy(platform)
            for name in ("aarch64-darwin", "x86_64-linux", "aarch64-linux")
        }
        for name in ("aarch64-darwin", "aarch64-linux"):
            (self.root / ("elastos-" + name)).write_bytes(self.binary)
            self.manifest["files"]["elastos-" + name] = copy.deepcopy(self.manifest["files"]["elastos-x86_64-linux"])
        self.approve_manifest()
        prepared = self.prepare()
        self.assertEqual(set(S.parse_json(prepared.release)["platforms"]),
                         {"aarch64-darwin", "x86_64-linux", "aarch64-linux"})

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
        (self.root / "elastos-x86_64-linux").write_bytes(b"x" * len(self.binary))
        with self.assertRaisesRegex(ValueError, "artifact differs"):
            self.prepare()

    def test_symlink_hardlink_directory_and_escape_are_refused(self):
        path = self.root / "elastos-x86_64-linux"
        path.unlink()
        path.symlink_to(self.root / "components-x86_64-linux.json")
        with self.assertRaises(OSError):
            self.prepare()
        for copied in self.snapshot.iterdir():
            copied.unlink()
        path.unlink()
        os.link(self.root / "components-x86_64-linux.json", path)
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
        with mock.patch.object(S.shutil, "disk_usage", return_value=SimpleNamespace(total=1000, free=1)):
            with self.assertRaisesRegex(ValueError, "needs more free space than the volume has"):
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
        with self.assertRaisesRegex(ValueError, "reference differs|Runtime differs"):
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
            self.assertFalse(S.confirmed(prepared.publisher_did, "approved release", io.StringIO(response), io.StringIO()))
            backend.assert_not_called()
        self.assertTrue(S.confirmed(prepared.publisher_did, "approved release", io.StringIO(DID + "\n"), io.StringIO()))

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
        self.policy["tool"]["path"] = str(self.root / "elastos-x86_64-linux")
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
        self.manifest["files"]["elastos-x86_64-linux"]["cid"] = v0
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"]["cid"] = v0
        self.approve_manifest()
        prepared = self.prepare()
        publication = dict(S.sign_publication(prepared, FakeBackend()))
        self.assertEqual(S.parse_json(publication["release.json"])["payload"]["platforms"]["x86_64-linux"]["binary"]["cid"], v0)
        self.assertNotEqual(root_digest, hashlib.sha256(self.binary).digest())
        (self.root / "elastos-x86_64-linux").write_bytes(b"x" * len(self.binary))
        fresh_snapshot = self.base / "tampered-snapshot"
        fresh_snapshot.mkdir(mode=0o700)
        with self.assertRaisesRegex(ValueError, "artifact differs"):
            S.prepare(self.policy, self.root, "signing-input.json", self.fetch, fresh_snapshot)

    def test_raw_cid_mismatch_and_changed_approved_cid_reference_are_refused(self):
        self.manifest["files"]["elastos-x86_64-linux"]["cid"] = S.raw_cid(b"other bytes")
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "raw CID differs"):
            self.prepare()
        for path in self.snapshot.iterdir():
            path.unlink()
        self.manifest["files"]["elastos-x86_64-linux"]["cid"] = S.raw_cid(self.binary)
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
        self.reapprove_components(components)
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
        self.platform_components["x86_64-linux"] = components
        self.manifest["release"]["platforms"]["x86_64-linux"]["binary"] = copy.deepcopy(self.manifest["files"]["elastos-x86_64-linux"])
        self.approve_manifest()

    def rewrite_signing_input(self):
        data = S.json_bytes(self.manifest)
        (self.root / "signing-input.json").write_bytes(data)
        self.policy["manifest_sha256"] = S.sha256(data)

    def test_generated_components_must_be_derived_from_attested_inputs(self):
        self.current_shape()
        name = "components-x86_64-linux.json"
        derived = S.parse_json((self.root / name).read_bytes())
        for change in ({"schema": "invalid-component-schema"}, {"profiles": {"extra": {}}},
                       {"external": {**derived["external"], "unlisted": {"platforms": {}}}}):
            data = S.json_bytes({**derived, **change})
            (self.root / name).write_bytes(data)
            record = {"sha256": S.sha256(data), "size": len(data), "cid": S.raw_cid(data)}
            self.manifest["files"][name] = record
            self.manifest["release"]["platforms"]["x86_64-linux"]["components"] = copy.deepcopy(record)
            self.rewrite_signing_input()
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "release components differ from attested inputs"):
                self.prepare()

    def test_platform_components_and_template_must_match_attested_receipt(self):
        (self.root / "provenance/x86_64-linux.components.json").write_bytes(
            S.json_bytes({"schema": "elastos.components/v1", "external": {"localhost-provider": {"platforms": {"*": {"url": "x"}}}}}))
        with self.assertRaisesRegex(ValueError, "platform components differ from attested receipt"):
            self.prepare()
        self.source_components = S.json_bytes({"external": {"localhost-provider": {}}, "profiles": {"other": {}}})
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "attested components template differs from approved source"):
            self.prepare()

    def test_model_catalog_needs_attested_bytes(self):
        self.current_shape()
        self.unattested.add("model-catalog.json")
        self.approve_manifest()
        with self.assertRaisesRegex(ValueError, "lack approved producer proof: model-catalog.json"):
            self.prepare()

    def test_single_chunk_unixfs_metadata_matches_existing_public_runtime_vector(self):
        data = b'{"payload":{"version":"0.7.1-rc.2"}}\n'
        expected = "QmZFMnZjkiTqy9VY5FDdKudvpsKtKxifk1poBrV7k4sqp8"
        self.assertEqual(S.unixfs_metadata_cid(data), expected)
        self.assertNotEqual(S.unixfs_metadata_cid(data[:-1]), expected)
        for value in (b"", b"x" * (256 * 1024 + 1)):
            with self.assertRaises(ValueError):
                S.unixfs_metadata_cid(value)

    def test_each_released_archive_is_signed_on_its_own_and_listed_at_the_release_top_level(self):
        self.current_shape()
        backend = FakeBackend()
        publication = dict(S.sign_publication(self.prepare(), backend))
        release = S.parse_json(publication["release.json"])["payload"]
        files = self.manifest["files"]
        def expected(name):
            return {"schema": "elastos.capsule.record/v1", "name": "localhost-provider", "platform": platform[name],
                    "capsule_version": None, "sha256": files[name]["sha256"], "size": files[name]["size"],
                    "cid": files[name]["cid"], "release_path": name, "signer_did": DID}
        platform = {"localhost-provider-capsule-metadata.tar.gz": "*", "localhost-provider-linux-amd64": "linux-amd64"}
        records = release["capsule_records"]
        self.assertEqual([record["payload"] for record in records], [expected(name) for name in sorted(platform)])
        self.assertEqual({record["signer_did"] for record in records}, {DID})
        signed = [call[1] for call in backend.calls if isinstance(call, tuple) and call[0] == "sign"]
        record_digests = [hashlib.sha256(b"elastos.capsule.record.v1\0" + S.json_bytes(record["payload"])).digest()
                          for record in records]
        # Each record has its own signature, made before the release that lists it.
        self.assertEqual(signed[:2], record_digests)
        self.assertEqual(len(signed), 4)
        self.assertEqual(set(release["platforms"]["x86_64-linux"]), {"binary", "components"})

    def test_unchanged_capsule_keeps_byte_identical_record_across_releases(self):
        self.current_shape()
        bound = S.parse_json((self.root / "components-x86_64-linux.json").read_bytes())
        def sign(domain, payload):
            return hashlib.sha512(domain.encode() + b"\0" + S.json_bytes(payload)).digest()
        def records(components, version):
            payloads = S.capsule_record_payloads(components, DID, version)
            release = S.parse_json(S.signed_release_bytes({"version": version}, payloads, DID, sign))
            return [S.json_bytes(record) for record in release["payload"]["capsule_records"]]
        first, second = records(bound, "1.2.3"), records(bound, "1.2.4")
        self.assertEqual(first, second)
        changed = copy.deepcopy(bound)
        data = b"changed provider bytes"
        changed["external"]["localhost-provider"]["platforms"]["linux-amd64"].update(
            checksum="sha256:" + S.sha256(data), size=len(data), cid=S.raw_cid(data))
        third = records(changed, "1.2.4")
        # Sorted by release path: metadata archive first, then the provider binary.
        self.assertEqual(third[0], first[0])
        self.assertNotEqual(third[1], first[1])

    def test_capsule_records_refuse_rows_a_home_could_not_verify(self):
        self.current_shape()
        bound = S.parse_json((self.root / "components-x86_64-linux.json").read_bytes())
        def row(components):
            return components["external"]["localhost-provider"]["platforms"]["linux-amd64"]
        cases = (
            ("must not be the release version", lambda c: c["external"]["localhost-provider"].update(version="1.2.3")),
            ("checksum refused", lambda c: row(c).update(checksum="sha512:" + "0" * 128)),
            ("size refused", lambda c: row(c).update(size=True)),
            ("size refused", lambda c: row(c).update(size=0)),
            ("CID refused", lambda c: row(c).update(cid=None)),
            ("unsafe relative path", lambda c: row(c).update(release_path="../escape")),
            ("conflicting capsule record", lambda c: c["external"]["localhost-provider"]["platforms"].update(
                {"linux-arm64": {**row(c), "size": row(c)["size"] + 1}})),
        )
        for message, change in cases:
            components = copy.deepcopy(bound)
            change(components)
            with self.subTest(message=message), self.assertRaisesRegex(ValueError, message):
                S.capsule_record_payloads(components, DID, "1.2.3")

    def test_records_beyond_one_chunk_are_refused_before_key_use(self):
        def payload(index):
            data = str(index).encode()
            return {"schema": "elastos.capsule.record/v1", "name": f"content-block-graph-provider-{index:03}",
                    "platform": "darwin-arm64", "capsule_version": None, "sha256": S.sha256(data), "size": 1,
                    "cid": "Qm" + "a" * 44, "release_path": f"content-block-graph-provider-{index:03}-capsule-metadata.tar.gz",
                    "signer_did": DID}
        release = S.json_bytes(self.manifest["release"])
        # 0.8.0-alpha.14 published 123 distinct archives; records leave room for twice that.
        fits = S.Prepared(DID, (), release, 2, None, tuple(payload(index) for index in range(250)))
        self.assertIn("release.json", dict(S.sign_publication(fits, FakeBackend())))
        backend = FakeBackend()
        too_many = S.Prepared(DID, (), release, 2, None, tuple(payload(index) for index in range(450)))
        with self.assertRaisesRegex(ValueError, "single-chunk release metadata required"):
            S.sign_publication(too_many, backend)
        self.assertEqual(backend.calls, [])

# Signed by the --model-catalog mode with a disposable key; the Runtime verifies
# these exact bytes in capsule_inventory.rs.
CATALOG_FIXTURE = SOURCE.parent.parent / "elastos/crates/elastos-server/tests/fixtures/model-catalog.json"
CATALOG_HEAD = "bafkreifmbqk5trnjxqfjiop5bzmml6vl5ybrpoujssrmfqce4wrodn4x4y"
NOW = 1_760_000_000
# A capsule record signed with a disposable key; the Runtime verifies these exact
# bytes in capsule_record.rs.
RECORD_FIXTURE = SOURCE.parent.parent / "elastos/crates/elastos-server/tests/fixtures/capsule-record.json"


def did_public(did):
    number = 0
    for char in did[len("did:key:z"):]:
        number = number * 58 + S.BASE58.index(char)
    return number.to_bytes(34, "big")[2:]


def catalog_entry(name, seed, **extra):
    capsule = {"schema": "elastos.capsule/v1", "version": "0.1.0", "name": name, "role": "content", "type": "data",
               "entrypoint": "weights.gguf", "projections": ["content"],
               "model_content": {"format": "gguf", "quantization": "Q4_K_M", "engine": "llama.cpp",
                                 "consumer_interface": "elastos.provider.model", "consumer_interface_version": "0.1.0",
                                 "minimum_memory_mb": 8192, "license": {"spdx_id": "Apache-2.0", "path": "LICENSE"},
                                 "provenance": {"base_repository": "fixture/base", "base_revision": "a" * 40,
                                                "base_license": {"spdx_id": "Apache-2.0", "path": "LICENSE.base"},
                                                "quantized_repository": "fixture/quantized", "quantized_revision": "b" * 40,
                                                "path": "PROVENANCE.md"}}}
    capsule.update(extra)
    contents = (("LICENSE", b"fixture license"), ("LICENSE.base", b"fixture base license"), ("PROVENANCE.md", b"fixture provenance"),
                ("capsule.json", S.json_bytes(capsule)), ("weights.gguf", b"GGUF fixture metadata only"))
    files = [{"path": path, "size": len(data), "sha256": S.sha256(data)} for path, data in contents]
    digest = hashlib.sha256("".join(f"{f['path']}\0{f['sha256']}\0{f['size']}\0" for f in files).encode()).hexdigest()
    cid = "b" + base64.b32encode(b"\x01\x70\x12\x20" + hashlib.sha256(seed.encode()).digest()).decode().lower().rstrip("=")
    return {"cid": cid, "capsule_manifest": capsule,
            "object_manifest": {"schema": "elastos.content.object.manifest/v1", "kind": "capsule",
                                "content_digest": "sha256:" + digest, "files": files}}


class ModelCatalogTests(unittest.TestCase):
    def setUp(self):
        self.fixture = CATALOG_FIXTURE.read_bytes()
        envelope = json.loads(self.fixture)
        self.did, self.payload = envelope["signer_did"], envelope["payload"]
        self.backend = FakeBackend()
        self.backend.public = did_public(self.did)
        self.backend.signature = bytes.fromhex(envelope["signature"])

    def test_signs_runtime_verified_fixture_byte_for_byte(self):
        catalog = S.sign_model_catalog(self.payload, self.did, self.backend, NOW)
        self.assertEqual(catalog, self.fixture)
        self.assertEqual(S.raw_cid(catalog), CATALOG_HEAD)
        digest = hashlib.sha256(b"elastos.model.catalog.v1\0" + S.json_bytes(self.payload)).digest()
        self.assertEqual(self.backend.calls[1:], [("sign", digest), ("verify", digest, self.backend.signature)])

    def test_refuses_payloads_the_runtime_refuses_before_any_key_use(self):
        def entry(payload):
            return payload["entries"][0]
        def capsule(payload):
            return entry(payload)["capsule_manifest"]
        def model(payload):
            return capsule(payload)["model_content"]
        cases = (
            ("schema refused", lambda p: p.update(schema="elastos.model.catalog/v2")),
            ("1 to 8", lambda p: p.update(entries=[])),
            ("1 to 8", lambda p: p.update(entries=[catalog_entry(f"model-{i}", str(i)) for i in range(9)])),
            ("expires_at", lambda p: p.update(expires_at=NOW)),
            ("published_at", lambda p: p.update(published_at=NOW + 1)),
            ("unique canonical", lambda p: p["entries"].append(copy.deepcopy(entry(p)))),
            ("unique capsule names", lambda p: p["entries"].append(dict(copy.deepcopy(entry(p)), cid=catalog_entry("x", "x")["cid"]))),
            ("entry fields", lambda p: entry(p).update(extra=True)),
            ("DAG-PB", lambda p: entry(p).update(cid=S.raw_cid(b"raw"))),
            ("model capsule schema", lambda p: capsule(p).update(schema="elastos.capsule/v2")),
            ("model capsule manifest fields", lambda p: capsule(p).update(extra=True)),
            ("model capsule manifest fields", lambda p: capsule(p).update(execution="data")),
            ("model capsule manifest fields", lambda p: capsule(p).update(runtime_abi="data")),
            ("model capsule manifest fields", lambda p: capsule(p).pop("model_content")),
            ("execution authority", lambda p: capsule(p).update(role="app")),
            ("execution authority", lambda p: capsule(p).update(capabilities=["elastos://peer/*"])),
            ("execution authority", lambda p: capsule(p).update(projections=["web"])),
            ("name refused", lambda p: capsule(p).update(name="model fixture")),
            ("display facts", lambda p: capsule(p).update(description="bell\x07")),
            ("display facts", lambda p: capsule(p).update(author="x" * 129)),
            ("display facts", lambda p: capsule(p).update(version="0.1.0\n")),
            ("version must not be empty", lambda p: capsule(p).update(version=" ")),
            ("GGUF", lambda p: capsule(p).update(entrypoint="weights.bin")),
            ("model_content fields", lambda p: model(p).pop("quantization")),
            ("model_content fields", lambda p: model(p).update(extra=True)),
            ("unsupported model_content", lambda p: model(p).update(quantization="Q5_K_M")),
            ("unsupported model_content", lambda p: model(p).update(minimum_memory_mb=0)),
            ("Apache-2.0", lambda p: model(p)["license"].update(spdx_id="MIT")),
            ("canonical relative path", lambda p: model(p)["license"].update(path="../LICENSE")),
            ("owner/repository", lambda p: model(p)["provenance"].update(base_repository="base")),
            ("lowercase Git revision", lambda p: model(p)["provenance"].update(base_revision="A" * 40)),
            ("digest", lambda p: entry(p)["object_manifest"].update(content_digest="sha256:" + "0" * 64)),
            ("self-contained", lambda p: entry(p)["object_manifest"].update(object_did=DID)),
            ("capsule.json", lambda p: capsule(p).update(version="0.1.1")),
            ("publisher must match", lambda p: entry(p)["object_manifest"].update(publisher_did=DID)),
        )
        for reason, edit in cases:
            payload = copy.deepcopy(self.payload)
            edit(payload)
            backend = FakeBackend()
            with self.subTest(reason=reason), self.assertRaisesRegex(ValueError, reason):
                S.sign_model_catalog(payload, self.did, backend, NOW)
            self.assertEqual(backend.calls, [])

    def test_passive_optional_fields_and_eight_entries_sign(self):
        payload = dict(self.payload, entries=[catalog_entry(f"model-{i}", str(i)) for i in range(7)])
        payload["entries"].append(catalog_entry("model-7", "7", description="Small model", author="Elastos",
                                                projections=["content"], requires=[]))
        catalog = S.sign_model_catalog(payload, self.did, self.backend, NOW)
        self.assertEqual(json.loads(catalog)["payload"], payload)

    def test_signed_size_bound_is_enforced_before_key_use(self):
        with mock.patch.object(S, "MAX_MODEL_CATALOG", len(self.fixture)):
            self.assertEqual(S.sign_model_catalog(self.payload, self.did, self.backend, NOW), self.fixture)
        backend = FakeBackend()
        with mock.patch.object(S, "MAX_MODEL_CATALOG", len(self.fixture) - 1), self.assertRaisesRegex(ValueError, "128 KiB"):
            S.sign_model_catalog(self.payload, self.did, backend, NOW)
        self.assertEqual(backend.calls, [])

    def cli(self, base, policy, typed):
        (base / "policy.json").write_bytes(S.json_bytes(policy))
        argv = [str(SOURCE), "--policy", str(base / "policy.json"), "--input-root", str(base / "input"),
                "--model-catalog", "payload.json", "--output-root", str(base / "catalog")]
        prompt = io.StringIO()
        with mock.patch.object(sys, "argv", argv), mock.patch.object(S, "pinned_tools"), \
             mock.patch.object(sys, "stdin", io.StringIO(typed)), mock.patch.object(sys, "stderr", prompt), \
             mock.patch.object(S, "OpenSSLBackend") as backend, self.assertRaises(ValueError) as refused:
            S.main()
        backend.assert_not_called()
        self.assertFalse((base / "catalog").exists())
        return str(refused.exception), prompt.getvalue()

    def test_cli_requires_policy_approved_payload_and_typed_did_before_key_use(self):
        temp = tempfile.TemporaryDirectory(dir=SOURCE.parent.parent)
        self.addCleanup(temp.cleanup)
        base = Path(temp.name).resolve()
        (base / "input").mkdir()
        data = S.json_bytes(self.payload)
        (base / "input" / "payload.json").write_bytes(data)
        policy = {"publisher_did": self.did, "key_path": "/custodian/unopened-signing.pem"}
        for approval in (None, S.sha256(data + b" ")):
            policy["model_catalog_sha256"] = approval
            error, prompt = self.cli(base, policy, self.did + "\n")
            self.assertRegex(error, "SHA-256 required|policy approval")
            self.assertEqual(prompt, "")
        policy["model_catalog_sha256"] = S.sha256(data)
        for typed in ("\n", "did:key:wrong\n", DID + "\n"):
            error, prompt = self.cli(base, policy, typed)
            self.assertIn("cancelled", error)
            self.assertIn(S.sha256(data), prompt)

    def test_real_openssl_cli_round_trip_verifies_against_the_did(self):
        openssl = shutil.which("openssl")
        if openssl is None:
            self.skipTest("openssl not installed")
        openssl = str(Path(openssl).resolve())
        if not subprocess.run([openssl, "version"], capture_output=True, text=True).stdout.startswith("OpenSSL 3."):
            self.skipTest("OpenSSL 3 required")
        temp = tempfile.TemporaryDirectory(dir=SOURCE.parent.parent)
        self.addCleanup(temp.cleanup)
        base = Path(temp.name).resolve()
        (base / "key").mkdir(mode=0o700)
        (base / "input").mkdir()
        key = base / "key" / "throwaway.pem"
        subprocess.run([openssl, "genpkey", "-algorithm", "ed25519", "-out", str(key)], check=True, capture_output=True)
        key.chmod(0o600)
        der = subprocess.run([openssl, "pkey", "-in", str(key), "-pubout", "-outform", "DER"], check=True, capture_output=True).stdout
        did = S.public_did(der[12:])
        data = S.json_bytes(self.payload)
        (base / "input" / "payload.json").write_bytes(data)
        python = Path(sys.executable).resolve()
        pin = lambda path: {"path": str(path), "sha256": S.sha256(Path(path).read_bytes())}
        (base / "policy.json").write_bytes(S.json_bytes({
            "publisher_did": did, "model_catalog_sha256": S.sha256(data), "key_path": str(key),
            "tool": pin(SOURCE), "python": pin(python), "openssl": pin(openssl)}))
        result = subprocess.run([str(python), "-I", "-S", str(SOURCE), "--policy", str(base / "policy.json"),
                                 "--input-root", str(base / "input"), "--model-catalog", "payload.json",
                                 "--output-root", str(base / "catalog")],
                                input=did + "\n", env={}, capture_output=True, text=True, timeout=60)
        self.assertEqual(result.returncode, 0, result.stderr)
        catalog = (base / "catalog" / "model-catalog.json").read_bytes()
        self.assertIn(S.raw_cid(catalog), result.stdout)
        envelope = json.loads(catalog)
        self.assertEqual((S.json_bytes(envelope), envelope["payload"], envelope["signer_did"]), (catalog, self.payload, did))
        # Verify with the key decoded from the signer DID alone, not the PEM.
        (base / "did.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + did_public(did))
        (base / "digest").write_bytes(hashlib.sha256(b"elastos.model.catalog.v1\0" + S.json_bytes(self.payload)).digest())
        signature = bytes.fromhex(envelope["signature"])
        for candidate, expected in ((signature, 0), (bytes([signature[0] ^ 1]) + signature[1:], 1)):
            (base / "signature").write_bytes(candidate)
            verify = subprocess.run([openssl, "pkeyutl", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", "did.der",
                                     "-in", "digest", "-sigfile", "signature"], cwd=base, capture_output=True)
            self.assertEqual(verify.returncode != 0, bool(expected))


class CapsuleRecordTests(unittest.TestCase):
    def test_signs_runtime_verified_fixture_byte_for_byte(self):
        fixture = RECORD_FIXTURE.read_bytes()
        envelope = json.loads(fixture)
        did, payload = envelope["signer_did"], envelope["payload"]
        components = {"external": {"system": {"install_path": "capsules/system", "platforms": {"*": {
            "checksum": "sha256:" + payload["sha256"], "size": payload["size"], "cid": payload["cid"],
            "release_path": "system.tar.gz", "install_path": "capsules/system", "extract_path": "system"}}}}}
        self.assertEqual(S.capsule_record_payloads(components, did, "0.8.0"), (payload,))
        backend = FakeBackend()
        backend.public = did_public(did)
        backend.signature = bytes.fromhex(envelope["signature"])
        self.assertEqual(S.signed_envelope(S.CAPSULE_RECORD_DOMAIN, payload, did, backend), fixture)
        digest = hashlib.sha256(b"elastos.capsule.record.v1\0" + S.json_bytes(payload)).digest()
        self.assertEqual(backend.calls, [("sign", digest), ("verify", digest, backend.signature)])


if __name__ == "__main__":
    unittest.main()
