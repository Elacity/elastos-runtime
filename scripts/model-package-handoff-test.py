#!/usr/bin/env python3
"""Behavior tests for scripts/model-package-handoff.py against pinned Kubo 0.40.1; stdlib only.

ELASTOS_TEST_KUBO_PATH=/path/to/kubo python3 scripts/model-package-handoff-test.py
"""

import contextlib
import hashlib
import http.client
import http.server
import importlib.util
import io
import json
import os
from pathlib import Path
import socket
import stat
import subprocess
import tempfile
import threading
import time
import unittest
from unittest import mock

HERE = Path(__file__).resolve().parent


def load_sibling(name):
    spec = importlib.util.spec_from_file_location(name.replace("-", "_"), HERE / f"{name}.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


helper = load_sibling("model-package-handoff")
bootstrap = load_sibling("install-bootstrap-test")

KUBO = os.environ.get("ELASTOS_TEST_KUBO_PATH")
KUBO_SKIP = "requires explicit pinned ELASTOS_TEST_KUBO_PATH"
OTHER_DID = bootstrap.PUBLISHER_DID
CACHE_BUDGET = 7 * 2**30
MEMORY_BUDGET = 5 * 2**30


def run_helper(*argv):
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        try:
            code = helper.main([str(arg) for arg in argv])
        except SystemExit as exit:
            code = exit.code
    return code, out.getvalue(), err.getvalue()


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(65536), b""):
            digest.update(chunk)
    return digest.hexdigest()


def unbound_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


def synthetic_closure(root):
    files = {
        "capsule.json": b'{"name":"synthetic-model","role":"content","type":"data"}\n',
        "LICENSE": b"Apache-2.0 (synthetic quantized model)\n",
        "LICENSE.base": b"Apache-2.0 (synthetic base model)\n",
        "PROVENANCE.md": b"# Provenance\n\nSynthetic bytes for the handoff test.\n",
        "weights.gguf": b"GGUF\x03\x00\x00\x00" + bytes((index * 7919 + 13) % 256 for index in range(600 * 1024)),
    }
    for name, data in files.items():
        (root / name).write_bytes(data)
    return sum(map(len, files.values()))


def signed_catalog(package_cid, published_at=1, expires_at=None):
    payload = {
        "schema": "elastos.model.catalog/v1",
        "published_at": published_at,
        "entries": [{
            "cid": package_cid,
            "capsule_manifest": {"name": "synthetic-model", "role": "content", "type": "data"},
            "object_manifest": {"files": ["weights.gguf"], "publisher_did": None},
        }],
    }
    if expires_at is not None:
        payload["expires_at"] = expires_at
    signed = bootstrap.sign_envelope(payload, "elastos.model.catalog.v1")
    return bootstrap.encode_envelope(signed), signed["signer_did"]


def fake_routes(**overrides):
    routes = {
        "/api/v0/version": (200, b'{"Version":"0.40.1"}\n'),
        "/api/v0/config?arg=Import.UnixFSHAMTDirectorySizeThreshold":
            (200, b'{"Key":"Import.UnixFSHAMTDirectorySizeThreshold","Value":null}\n'),
        "/api/v0/config?arg=Import.UnixFSHAMTDirectorySizeEstimation":
            (200, b'{"Key":"Import.UnixFSHAMTDirectorySizeEstimation","Value":null}\n'),
    }
    routes.update(overrides)
    return routes


class FakeKuboHandler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_POST(self):
        answer = self.server.routes.get(self.path)
        if callable(answer):
            answer(self)
            return
        status, body = answer or (404, b'{"Message":"no such route","Type":"error"}\n')
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


@contextlib.contextmanager
def fake_kubo(routes):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), FakeKuboHandler)
    server.daemon_threads = True
    server.routes = routes
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


class KuboDaemon:
    def __init__(self, repo):
        self.repo = Path(repo)
        self.env = {**os.environ, "IPFS_PATH": str(self.repo)}
        self.process = None
        self.port = None
        self.api_url = None

    def run(self, *args):
        completed = subprocess.run([KUBO, *map(str, args)], env=self.env, check=True, capture_output=True, text=True)
        return completed.stdout.strip()

    def start(self):
        self.repo.mkdir()
        self.run("init", "--empty-repo", "--profile=test")
        self.run("config", "Addresses.API", "/ip4/127.0.0.1/tcp/0")
        self.run("config", "Addresses.Gateway", "/ip4/127.0.0.1/tcp/0")
        self.run("config", "--json", "Addresses.Swarm", "[]")
        log_path = self.repo.parent / f"{self.repo.name}.log"
        with open(log_path, "wb") as log:
            self.process = subprocess.Popen([KUBO, "daemon", "--offline"], env=self.env, stdout=log, stderr=subprocess.STDOUT)
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline:
            if self.process.poll() is not None:
                raise RuntimeError(f"kubo daemon exited early:\n{log_path.read_text()}")
            api_file = self.repo / "api"
            if api_file.exists() and self._answers(int(api_file.read_text().strip().rsplit("/", 1)[1])):
                return self
            time.sleep(0.05)
        raise RuntimeError(f"kubo daemon did not become ready:\n{log_path.read_text()}")

    def _answers(self, port):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=2)
        try:
            connection.request("POST", "/api/v0/version")
            if connection.getresponse().status != 200:
                return False
        except OSError:
            return False
        finally:
            connection.close()
        self.port, self.api_url = port, f"http://127.0.0.1:{port}"
        return True

    def stop(self):
        if self.process is None or self.process.poll() is not None:
            return
        self.process.terminate()
        try:
            self.process.wait(timeout=15)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait()


class OfflineTest(unittest.TestCase):
    def test_head_cid_matches_recorded_kubo_vector(self):
        # Recorded from `kubo add --cid-version=1 --raw-leaves -Q -n` on Kubo 0.40.1 for these exact bytes.
        self.assertEqual(helper.raw_cid(b'{"a":1}\n'), "bafkreihdizbsainqif4vddmwctzvmdgnoe2uutxbaho4xcj5nfm2tvrqdq")


@unittest.skipUnless(KUBO and os.access(KUBO, os.X_OK), KUBO_SKIP)
class KuboHandoffTest(unittest.TestCase):
    def setUp(self):
        scratch = tempfile.TemporaryDirectory(prefix="model-package-handoff-")
        self.addCleanup(scratch.cleanup)
        self.root = Path(scratch.name)
        self.sender = KuboDaemon(self.root / "sender").start()
        self.addCleanup(self.sender.stop)
        self.receiver = KuboDaemon(self.root / "receiver").start()
        self.addCleanup(self.receiver.stop)
        self.closure = self.root / "closure"
        self.closure.mkdir()
        self.payload_bytes = synthetic_closure(self.closure)
        self.cid = self.sender.run("add", "-r", "--cid-version=1", "--raw-leaves", "--chunker=size-262144", "-Q", self.closure)
        self.assertTrue(self.cid.startswith("bafybei"), self.cid)
        self.other_cid = self.cid[:-1] + ("a" if self.cid[-1] != "a" else "b")

    def export_package(self):
        car = self.root / "package.car"
        code, out, err = run_helper("export", "--cid", self.cid, "--output", car, "--kubo-api", self.sender.api_url)
        self.assertEqual((code, err), (0, ""))
        return car, self.root / "package.car.receipt.json", json.loads(out)

    def dag_stat(self, node):
        return json.loads(node.run("dag", "stat", "--enc=json", "--progress=false", self.cid))["DagStats"][0]

    def recursive_pins(self, node):
        return sorted(node.run("pin", "ls", "--type=recursive").splitlines())

    def receiver_data_dir(self):
        data_dir = self.root / "receiver-data"
        data_dir.mkdir(mode=0o700)
        components = {
            "schema": "elastos.components/v1",
            "capsules": {"shell": {"cid": "", "sha256": "", "size": 0, "platforms": ["x86_64-linux"]}},
            "external": {
                "kubo": {"version": "0.40.1", "platforms": {"x86_64-linux": {
                    "url": "https://example.invalid/kubo.tar.gz", "checksum": "sha256:" + "a" * 64, "extract_path": "kubo/ipfs"}}},
                "llama-server": {"version": "b1", "platforms": {}},
                "carrier": {"version": "1", "platforms": {}},
            },
            "profiles": {"default": {"external": ["kubo"], "capsules": ["shell"]}},
            "model_catalog": {"head_cid": "bafkreiold", "publisher_dids": ["did:key:z6MkOld"]},
        }
        (data_dir / "components.json").write_bytes(json.dumps(components, indent=2).encode() + b"\n")
        return data_dir, components

    def pin_catalog(self, data_dir, catalog_path, head, did, package_cid=None):
        return run_helper(
            "pin-catalog", "--data-dir", data_dir, "--catalog", catalog_path, "--head-cid", head,
            "--publisher-did", did, "--publisher-did", OTHER_DID, "--package-cid", package_cid or self.cid,
            "--max-cache-bytes", CACHE_BUDGET, "--max-model-memory-bytes", MEMORY_BUDGET)

    @staticmethod
    def snapshot(data_dir):
        return {path.name: path.read_bytes() for path in data_dir.iterdir()}

    def test_verify_kubo_accepts_pinned_version_and_canonical_profile(self):
        code, out, err = run_helper("verify-kubo", "--kubo-api", self.sender.api_url)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(out), {"kubo_version": "0.40.1", "import_profile": "canonical"})

        data_dir = self.root / "sender-data"
        data_dir.mkdir()
        coords = {"kubo_pid": self.sender.process.pid, "api_port": self.sender.port, "gateway_port": 0, "started_at": 0, "last_used": 0}
        (data_dir / "ipfs-coords.json").write_text(json.dumps(coords))
        code, out, err = run_helper("verify-kubo", "--data-dir", data_dir)
        self.assertEqual((code, err, json.loads(out)["kubo_version"]), (0, "", "0.40.1"))

        with fake_kubo(fake_routes(**{"/api/v0/version": (200, b'{"Version":"0.39.0"}\n')})) as url:
            code, out, err = run_helper("verify-kubo", "--kubo-api", url)
        self.assertEqual((code, out, err), (1, "", "error: kubo version '0.39.0' is not 0.40.1\n"))

        threshold = "/api/v0/config?arg=Import.UnixFSHAMTDirectorySizeThreshold"
        with fake_kubo(fake_routes(**{threshold: (200, b'{"Key":"Import.UnixFSHAMTDirectorySizeThreshold","Value":"1MiB"}\n')})) as url:
            code, out, err = run_helper("verify-kubo", "--kubo-api", url)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("Import.UnixFSHAMTDirectorySizeThreshold='1MiB' is not canonical", err)

    def test_export_streams_car_with_receipt_and_verify_car_refuses_corruption(self):
        car = self.root / "package.car"
        receipt_path = self.root / "package.car.receipt.json"
        reads = []
        original_read = helper.TrailerResponse.read

        def counting_read(response, amt=None):
            reads.append(amt)
            return original_read(response, amt)

        with mock.patch.object(helper, "CHUNK_BYTES", 4096), mock.patch.object(helper.TrailerResponse, "read", counting_read):
            code, out, err = run_helper("export", "--cid", self.cid, "--output", car, "--kubo-api", self.sender.api_url)
        self.assertEqual((code, err), (0, ""))
        receipt = json.loads(out)
        size = car.stat().st_size
        self.assertEqual(receipt, {
            "schema": "elastos.model.package-car/v1", "package_cid": self.cid, "car_sha256": sha256_file(car),
            "car_bytes": size, "kubo_version": "0.40.1", "exported_at": receipt["exported_at"]})
        self.assertGreater(size, self.payload_bytes)
        self.assertGreaterEqual(reads.count(4096), size // 4096, "the CAR body was not read in 4 KiB chunks")
        self.assertEqual(json.loads(receipt_path.read_bytes()), receipt)
        self.assertFalse((self.root / "package.car.partial").exists())
        code, out, err = run_helper("verify-car", "--car", car, "--receipt", receipt_path, "--cid", self.cid)
        self.assertEqual((code, err, json.loads(out)), (0, "", receipt))

        truncated = self.root / "truncated.car"
        truncated.write_bytes(car.read_bytes()[:-1000])
        flipped_bytes = bytearray(car.read_bytes())
        flipped_bytes[size // 2] ^= 0xFF
        flipped = self.root / "flipped.car"
        flipped.write_bytes(flipped_bytes)
        other_receipt = self.root / "other.receipt.json"
        other_receipt.write_text(json.dumps({**receipt, "package_cid": self.other_cid}))
        dead_kubo = f"http://127.0.0.1:{unbound_port()}"
        cases = [
            ("truncated", truncated, receipt_path, f"CAR size {size - 1000} does not match receipt car_bytes {size}"),
            ("flipped byte", flipped, receipt_path, "does not match receipt car_sha256"),
            ("other receipt", car, other_receipt, f"receipt package_cid {self.other_cid} does not match expected CID {self.cid}"),
        ]
        for label, car_path, receipt_file, expected in cases:
            with self.subTest(label):
                code, out, err = run_helper("verify-car", "--car", car_path, "--receipt", receipt_file, "--cid", self.cid)
                self.assertEqual((code, out), (1, ""))
                self.assertIn(expected, err)
                code, out, err = run_helper(
                    "import", "--car", car_path, "--receipt", receipt_file, "--cid", self.cid,
                    "--free-space-floor-bytes", 0, "--kubo-api", dead_kubo)
                self.assertEqual((code, out), (1, ""))
                self.assertIn(expected, err)
                self.assertNotIn("unreachable", err, "import contacted Kubo before the CAR gate")

        def failing_export(handler):
            handler.send_response(200)
            handler.send_header("Transfer-Encoding", "chunked")
            handler.send_header("Trailer", "X-Stream-Error")
            handler.end_headers()
            handler.wfile.write(b"5\r\nCARv1\r\n0\r\nX-Stream-Error: block bafkreimissing not found\r\n\r\n")

        with fake_kubo(fake_routes(**{f"/api/v0/dag/export?arg={self.cid}": failing_export})) as url:
            code, out, err = run_helper("export", "--cid", self.cid, "--output", self.root / "failed.car", "--kubo-api", url)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("failed mid-stream: block bafkreimissing not found", err)
        self.assertEqual(sorted(path.name for path in self.root.glob("failed.car*")), [])

    def test_import_pins_complete_root_on_receiver_and_is_idempotent(self):
        car, receipt_path, _ = self.export_package()
        code, out, err = run_helper("verify-kubo", "--kubo-api", self.receiver.api_url)
        self.assertEqual((code, err), (0, ""))
        base = ["import", "--car", car, "--receipt", receipt_path, "--cid", self.cid, "--kubo-api", self.receiver.api_url]

        code, out, err = run_helper(*base, "--free-space-floor-bytes", 2**62)
        self.assertEqual((code, out), (1, ""))
        self.assertIn(f"leave less than the {2**62}-byte --free-space-floor-bytes floor", err)
        self.assertNotIn(f"{self.cid} recursive", self.recursive_pins(self.receiver))

        code, out, err = run_helper(*base, "--free-space-floor-bytes", 0)
        self.assertEqual((code, err), (0, ""))
        first = json.loads(out)
        sender_stat = self.dag_stat(self.sender)
        self.assertEqual(first, {
            "schema": "elastos.model.package-import/v1", "package_cid": self.cid, "pinned": True,
            "blocks": sender_stat["NumBlocks"], "block_bytes": sender_stat["Size"], "kubo_version": "0.40.1"})
        pins = self.recursive_pins(self.receiver)
        self.assertIn(f"{self.cid} recursive", pins)
        self.assertEqual(self.dag_stat(self.receiver), sender_stat)
        fetched = self.root / "fetched"
        self.receiver.run("get", "-o", fetched, self.cid)
        self.assertEqual(sorted(path.name for path in fetched.iterdir()), sorted(path.name for path in self.closure.iterdir()))
        for path in self.closure.iterdir():
            self.assertEqual((fetched / path.name).read_bytes(), path.read_bytes(), path.name)

        code, out, err = run_helper(*base, "--free-space-floor-bytes", 0)
        self.assertEqual((code, err, json.loads(out)), (0, "", first))
        self.assertEqual(self.recursive_pins(self.receiver), pins)

    def test_pin_catalog_patches_only_model_catalog_and_refuses_timed_snapshots(self):
        data_dir, components = self.receiver_data_dir()
        catalog, did = signed_catalog(self.cid)
        catalog_path = self.root / "model-catalog.json"
        catalog_path.write_bytes(catalog)
        head = helper.raw_cid(catalog)
        self.assertEqual(head, self.sender.run("add", "--cid-version=1", "--raw-leaves", "-Q", "-n", catalog_path))
        code, out, err = run_helper("head-cid", "--catalog", catalog_path)
        self.assertEqual((code, err, json.loads(out)), (0, "", {"head_cid": head, "catalog_bytes": len(catalog)}))

        code, out, err = self.pin_catalog(data_dir, catalog_path, head, did)
        self.assertEqual((code, err), (0, ""))
        expected_pin = {"head_cid": head, "publisher_dids": [did, OTHER_DID],
                        "local_use": {"max_cache_bytes": CACHE_BUDGET, "max_model_memory_bytes": MEMORY_BUDGET}}
        self.assertEqual(json.loads(out), {
            "schema": "elastos.model.catalog-pin/v1", "head_cid": head, "package_cid": self.cid,
            **expected_pin, "signature_verification": "pending_runtime_read"})
        patched = json.loads((data_dir / "components.json").read_bytes())
        self.assertEqual(list(patched), list(components))
        self.assertEqual({key: value for key, value in patched.items() if key != "model_catalog"},
                         {key: value for key, value in components.items() if key != "model_catalog"})
        self.assertEqual(patched["model_catalog"], expected_pin)
        self.assertEqual((data_dir / "model-catalog.json").read_bytes(), catalog)
        for name in ("model-catalog.json", "components.json"):
            info = (data_dir / name).stat()
            self.assertEqual((stat.S_IMODE(info.st_mode), info.st_nlink), (0o600, 1), name)
        snapshot = self.snapshot(data_dir)
        self.assertEqual(sorted(snapshot), ["components.json", "model-catalog.json"])

        timed, _ = signed_catalog(self.cid, expires_at=4000000000)
        timed_path = self.root / "timed-catalog.json"
        timed_path.write_bytes(timed)
        refusals = [
            ("timed snapshot", (timed_path, helper.raw_cid(timed), did, None),
             "expires_at=4000000000 is a timed snapshot; this helper pins permanent snapshots only"),
            ("wrong head", (catalog_path, helper.raw_cid(b"other bytes"), did, None), f"catalog head is {head}, not --head-cid"),
            ("entry cid", (catalog_path, head, did, self.other_cid), f"does not match --package-cid {self.other_cid}"),
            ("unknown signer", (catalog_path, head, "did:key:z6MkUnknownSigner", None), "is not among the given --publisher-did values"),
        ]
        for label, arguments, expected in refusals:
            with self.subTest(label):
                code, out, err = self.pin_catalog(data_dir, *arguments)
                self.assertEqual((code, out), (1, ""))
                self.assertIn(expected, err)
                self.assertEqual(self.snapshot(data_dir), snapshot)
        data_dir.chmod(0o770)
        try:
            code, out, err = self.pin_catalog(data_dir, catalog_path, head, did)
        finally:
            data_dir.chmod(0o700)
        self.assertEqual((code, out), (1, ""))
        self.assertIn("not writable by group or others", err)
        self.assertEqual(self.snapshot(data_dir), snapshot)

    def test_two_pins_stay_separate(self):
        car, receipt_path, _ = self.export_package()
        code, out, err = run_helper(
            "import", "--car", car, "--receipt", receipt_path, "--cid", self.cid,
            "--free-space-floor-bytes", 0, "--kubo-api", self.receiver.api_url)
        self.assertEqual((code, err), (0, ""))
        data_dir, _ = self.receiver_data_dir()
        components_path = data_dir / "components.json"
        catalog, did = signed_catalog(self.cid)
        catalog_path = self.root / "model-catalog.json"
        catalog_path.write_bytes(catalog)
        code, out, err = self.pin_catalog(data_dir, catalog_path, helper.raw_cid(catalog), did)
        self.assertEqual((code, err), (0, ""))
        pinned = json.loads(components_path.read_bytes())["model_catalog"]

        self.receiver.run("pin", "rm", self.cid)
        pins = self.recursive_pins(self.receiver)
        self.assertNotIn(f"{self.cid} recursive", pins)
        self.assertEqual(json.loads(components_path.read_bytes())["model_catalog"], pinned)

        second, _ = signed_catalog(self.cid, published_at=2)
        second_path = self.root / "second-catalog.json"
        second_path.write_bytes(second)
        code, out, err = self.pin_catalog(data_dir, second_path, helper.raw_cid(second), did)
        self.assertEqual((code, err), (0, ""))
        self.assertEqual(json.loads(components_path.read_bytes())["model_catalog"]["head_cid"], helper.raw_cid(second))
        self.assertNotEqual(helper.raw_cid(second), pinned["head_cid"])
        self.assertEqual(self.recursive_pins(self.receiver), pins)


class RepoCatalogPinTests(unittest.TestCase):
    def test_repo_catalog_matches_components_pin(self):
        root = HERE.parent
        catalog = (root / "model-catalog.json").read_bytes()
        components = json.loads((root / "components.json").read_bytes())
        pin = components["model_catalog"]
        self.assertEqual(helper.raw_cid(catalog), pin["head_cid"])
        signed = json.loads(catalog)
        self.assertNotIn("expires_at", signed["payload"])
        self.assertEqual(len(signed["payload"]["entries"]), 2)
        self.assertEqual(
            signed["payload"]["entries"][0]["cid"],
            "bafybeid5l7gfgsqy2wozia2q7mtyux2wrbnlfehzz4at3ic3cngvyku6hi",
        )
        self.assertEqual(
            signed["payload"]["entries"][1]["cid"],
            "bafybeidy5kfvqwg6g6pfgdfwslmhijosbeskt5b2duqdqxnc7e6fwmr72y",
        )
        self.assertIn(signed["signer_did"], pin["publisher_dids"])
        helper.check_catalog(
            catalog,
            signed["payload"]["entries"][0]["cid"],
            pin["publisher_dids"],
        )
        helper.check_catalog(
            catalog,
            signed["payload"]["entries"][1]["cid"],
            pin["publisher_dids"],
        )
        self.assertGreaterEqual(pin["local_use"]["max_cache_bytes"], 18516684377)
        self.assertGreaterEqual(pin["local_use"]["max_model_memory_bytes"], 8589934592)


if __name__ == "__main__":
    unittest.main(verbosity=2)
