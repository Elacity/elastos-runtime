#!/usr/bin/env python3
"""Small key-free build fixtures. Kubo receipts are mocked, not product proof."""

import base64
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import re
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch
import urllib.parse


WORKSPACE = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("upstream_assets", Path(__file__).with_name("release-upstream-assets.py"))
assets = importlib.util.module_from_spec(spec)
spec.loader.exec_module(assets)


def fixture_cid(data, codec=0x70):
    # Mock receipt identity only; this does not calculate a UnixFS directory.
    return "b" + base64.b32encode(bytes((1, codec, 0x12, 0x20)) + hashlib.sha256(data).digest()).decode().lower().rstrip("=")


class Connection:
    def __init__(self, kubo):
        self.kubo, self.headers, self.chunks = kubo, {}, []
        self.closed = False

    def putrequest(self, method, target):
        self.method, self.target = method, target

    def putheader(self, name, value):
        self.headers[name] = value

    def endheaders(self):
        pass

    def send(self, data):
        self.chunks.append(data)

    def getresponse(self):
        self.kubo.test.assertEqual(self.method, "POST")
        path, query = self.target.split("?", 1)
        self.kubo.test.assertEqual(path, "/api/v0/add")
        options = dict(urllib.parse.parse_qsl(query))
        only_hash = options.pop("only-hash") == "true"
        self.kubo.test.assertEqual(options.pop("pin"), str(not only_hash).lower())
        self.kubo.test.assertEqual(options, assets.MODEL_ADD_OPTIONS)
        body = b"".join(self.chunks)
        self.kubo.test.assertEqual(len(body), int(self.headers["Content-Length"]))
        boundary = self.headers["Content-Type"].split("boundary=", 1)[1].encode()
        files = {}
        for part in body.split(b"--" + boundary)[1:-1]:
            header, data = part.split(b"\r\n\r\n", 1)
            name = re.search(rb'filename="([^"]+)"', header)[1].decode()
            files[name] = data[:-2]
        self.kubo.uploads.append((only_hash, files))
        root = fixture_cid(b"".join(name.encode() + b"\0" + data for name, data in sorted(files.items())))
        if not only_hash:
            root = self.kubo.retained_override or root
            self.kubo.pins.add(root)
        if self.kubo.response_override:
            return self.kubo.response_override
        response = io.BytesIO(json.dumps({"Name": "", "Hash": root, "Size": "1"}).encode() + b"\n")
        response.status, response.trailers = 200, {}
        return response

    def close(self):
        self.closed = True


class Kubo:
    def __init__(self, test, repo):
        self.test, self.repo = test, repo
        self.base_url = "http://127.0.0.1:59101"
        self.uploads, self.connections, self.pins, self.exports = [], [], set(), []
        self.retained_override = self.response_override = None
        self.profile_error = self.export_error = False
        self.pin_verified = True

    def verify_profile(self):
        if self.profile_error:
            raise assets.handoff.Refusal("fixture noncanonical profile")
        return "0.40.1"

    def repo_path(self):
        return str(self.repo)

    def _connect(self):
        connection = Connection(self)
        self.connections.append(connection)
        return connection

    def is_recursively_pinned(self, cid):
        return self.pin_verified and cid in self.pins

    def export_car(self, cid, destination):
        self.exports.append(cid)
        data = b"inert CAR fixture for " + cid.encode()
        Path(destination).write_bytes(data)
        if self.export_error:
            raise assets.handoff.Refusal("fixture export stream error")
        return assets.handoff.CarDigest(hashlib.sha256(data).hexdigest(), len(data))


class AssetsTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.root = Path(self.scratch.name).resolve()
        self.output, self.cache, self.repo = self.root / "artifacts", self.root / "cache", self.root / "kubo-repo"
        self.repo.mkdir(mode=0o700)
        (self.repo / "api").write_text("/ip4/127.0.0.1/tcp/59101\n")
        self.bin = self.root / "qualified-kubo"
        self.bin.write_bytes(b"inert Kubo binary fixture")
        self.bin.chmod(0o700)
        self.recipes = [self.recipe("kubo", self.bin.read_bytes(), model=False)]
        for index, name in enumerate(sorted(assets.MODEL_COMPONENTS)):
            weight = b"GGUF" + bytes((index,)) * (assets.upstream.CHUNK + 7 if index == 0 else 23)
            self.recipes.append(self.recipe(name, weight, model=True))
        self.inventory = self.root / "recipes.json"
        self.write_inventory()
        self.kubo = Kubo(self, self.repo)
        self.patch("SOURCE_ROOT", self.root)
        self.patch("RECIPE_FILE", self.inventory)

    def patch(self, name, value):
        patcher = patch.object(assets, name, value)
        patcher.start()
        self.addCleanup(patcher.stop)

    def source(self, name, data):
        path = self.root / "inputs" / name
        path.parent.mkdir(exist_ok=True)
        path.write_bytes(data)
        return {"path": str(path.relative_to(self.root)), "checksum": "sha256:" + hashlib.sha256(data).hexdigest(), "max_bytes": len(data)}

    def recipe(self, name, data, model):
        recipe = {"schema": assets.upstream.SCHEMA, "component": name, "platform": "*" if model else "linux-amd64",
            "version": "1.0.0" if model else "0.40.1", "source": self.source(name + "-payload", data), "format": "raw",
            "root": name, "entrypoint": "weights.gguf" if model else "ipfs",
            "extract_path": name + ("/weights.gguf" if model else "/ipfs"), "install_path": "models/" + name if model else "bin/kubo",
            "max_unpacked_bytes": len(data), "license": {"spdx_id": "Apache-2.0", "files": [
                {"name": "LICENSE", "source": self.source(name + "-license", b"inert model license\n")} ]}}
        if model:
            recipe["license"]["files"].append({"name": "LICENSE.base", "source": self.source(name + "-base-license", b"inert base license\n")})
            recipe["notices"] = [{"name": "PROVENANCE.md", "source": self.source(name + "-provenance", b"inert provenance\n")}]
            recipe["model_content"] = {"format": "gguf", "quantization": "Q1_0" if "bonsai" in name else "Q4_K_M",
                "engine": "llama.cpp", "consumer_interface": "elastos.provider.model", "consumer_interface_version": "0.1.0",
                "minimum_memory_mb": 512, "license": {"spdx_id": "Apache-2.0", "path": "LICENSE"},
                "provenance": {"base_repository": "fixture/base", "base_revision": "a" * 40,
                    "quantized_repository": "fixture/quantized", "quantized_revision": "b" * 40,
                    "base_license": {"spdx_id": "Apache-2.0", "path": "LICENSE.base"}, "path": "PROVENANCE.md"}}
        return recipe

    def write_inventory(self):
        self.inventory.write_text(json.dumps({"schema": "elastos.release-upstream-recipes/v1", "recipes": self.recipes}))

    def prepare(self, **options):
        arguments = {"model_kubo_bin": self.bin, "model_kubo_repo": self.repo, "published_at": 123,
                     "model_publisher_did": "did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c", **options}
        with (patch.object(assets.subprocess, "run", return_value=SimpleNamespace(stdout="0.40.1\n")) as command,
              patch.object(assets.handoff, "KuboApi", return_value=self.kubo),
              patch.object(assets.handoff, "kubo_from_args", return_value=self.kubo)):
            result = assets.prepare("linux-amd64", self.cache, self.output, **arguments)
        self.assertEqual(command.call_args.args[0], [str(self.bin), "version", "--number"])
        self.assertEqual(command.call_args.kwargs["env"]["IPFS_PATH"], str(self.repo))
        return result

    def model_package(self):
        self.output.mkdir(mode=0o700)
        recipe = assets.resolved_recipe(self.recipes[1])
        receipt = assets.upstream.package(recipe, self.cache, self.output)
        return recipe, receipt

    def test_default_driver_keeps_existing_manifest_and_output_shape(self):
        with patch.object(assets.handoff, "KuboApi") as api, patch.object(assets.subprocess, "run") as command:
            result = assets.prepare("linux-amd64", self.cache, self.output)
        api.assert_not_called()
        command.assert_not_called()
        self.assertEqual(set(result), {"external"})
        self.assertTrue(all(set(component) == {"platforms", "capsule_metadata"} for component in result["external"].values()))
        receipt = json.loads((self.output / "upstream-input.json").read_bytes())
        self.assertEqual(set(receipt), {"schema", "platform", "recipes_sha256", "capsules"})
        self.assertFalse((self.root / "artifacts-model-handoff").exists())

    def test_four_entry_draft_preserves_exact_index_and_uses_separate_retention_files(self):
        result = self.prepare(model_publisher_did="did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c")
        handoff_dir = self.root / "artifacts-model-handoff"
        draft = json.loads((handoff_dir / "model-catalog.unsigned.json").read_bytes())
        self.assertEqual(set(draft), {"schema", "published_at", "expires_at", "entries"})
        self.assertEqual(draft["schema"], "elastos.model.catalog/v1")
        self.assertEqual(draft["published_at"], 123)
        self.assertIsNone(draft["expires_at"])
        self.assertEqual(len(draft["entries"]), 4)
        self.assertEqual(len({entry["cid"] for entry in draft["entries"]}), 4)
        self.assertEqual(len(self.kubo.uploads), 8)
        self.assertEqual(len(self.kubo.exports), 4)
        self.assertTrue(all(connection.closed for connection in self.kubo.connections))
        self.assertTrue(all(max(map(len, connection.chunks)) <= assets.upstream.CHUNK for connection in self.kubo.connections))
        for index, entry in enumerate(draft["entries"]):
            self.assertEqual(set(entry), {"cid", "capsule_manifest", "object_manifest"})
            self.assertEqual(assets.directory_cid(entry["cid"]), entry["cid"])
            name = entry["capsule_manifest"]["name"]
            first, second = self.kubo.uploads[index * 2:index * 2 + 2]
            self.assertEqual((first[0], second[0]), (True, False))
            self.assertEqual(first[1], second[1])
            self.assertEqual(first[1]["capsule.json"], assets.upstream.canonical(entry["capsule_manifest"])[:-1])
            self.assertEqual(first[1]["_elastos_object.json"], assets.upstream.canonical(entry["object_manifest"]))
            for item in entry["object_manifest"]["files"]:
                data = first[1][item["path"]]
                self.assertEqual((item["size"], item["sha256"]), (len(data), hashlib.sha256(data).hexdigest()))
            retention = result["model_retention"][name]
            self.assertEqual(set(retention), {"package_cid", "car", "receipt"})
            self.assertEqual(retention["package_cid"], entry["cid"])
            for descriptor in (retention["car"], retention["receipt"]):
                path = handoff_dir / descriptor["release_path"]
                self.assertEqual(set(descriptor), {"release_path", "checksum", "size"})
                self.assertEqual(descriptor["checksum"], "sha256:" + hashlib.sha256(path.read_bytes()).hexdigest())
                self.assertEqual(descriptor["size"], path.stat().st_size)
                self.assertFalse((self.output / path.name).exists())
            assets.handoff.check_car(handoff_dir / retention["car"]["release_path"], handoff_dir / retention["receipt"]["release_path"], entry["cid"])
            self.assertEqual(set(result["external"][name]), {"platforms", "capsule_metadata"})
        self.assertFalse(list(self.output.glob(".model-closure-*")))
        self.assertEqual(result["model_catalog_unsigned"]["publisher_did"], "did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c")
        build_record = json.loads((self.output / "upstream-input.json").read_bytes())
        self.assertEqual(build_record["model_retention"], result["model_retention"])

    def test_kubo_options_match_runtime_canonical_directory_options(self):
        source = (WORKSPACE / "capsules/ipfs-provider/src/directory_hash.rs").read_text()
        block = source.split("const ADD_OPTIONS:", 1)[1].split("];", 1)[0]
        expected = dict(re.findall(r'\("([^"]+)", "([^"]+)"\)', block))
        self.assertEqual(expected, {**assets.MODEL_ADD_OPTIONS, "only-hash": "true", "pin": "false"})

    def test_incomplete_options_invalid_timestamp_and_model_set_refused_before_packaging(self):
        with patch.object(assets.upstream, "package") as package:
            for options in ({"model_kubo_bin": self.bin}, {"model_publisher_did": "did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c"},
                    {"model_kubo_bin": self.bin, "model_kubo_repo": self.repo, "published_at": -1},
                    {"model_kubo_bin": self.bin, "model_kubo_repo": self.repo, "published_at": True}):
                with self.subTest(options=options), self.assertRaises(ValueError):
                    assets.prepare("linux-amd64", self.cache, self.output, **options)
            self.recipes.pop()
            self.write_inventory()
            with self.assertRaisesRegex(ValueError, "four pinned"):
                assets.prepare("linux-amd64", self.cache, self.output, model_kubo_bin=self.bin, model_kubo_repo=self.repo, published_at=123,
                    model_publisher_did="did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c")
            package.assert_not_called()

    def test_fresh_model_preparation_requires_a_canonical_public_did_before_packaging(self):
        with patch.object(assets.upstream, "package") as package:
            for publisher in (None, "did:key:z6MkFixture", "did:key:z" + "1" * 48, 123):
                with self.subTest(publisher=publisher), self.assertRaises(ValueError):
                    self.prepare(model_publisher_did=publisher)
            package.assert_not_called()
        self.assertFalse(self.output.exists())

    def test_missing_source_pin_refused_before_kubo_or_network(self):
        self.recipes[0]["source"].pop("checksum")
        self.write_inventory()
        with patch.object(assets.handoff, "KuboApi") as api, patch.object(assets.upstream, "response") as network:
            with self.assertRaisesRegex(ValueError, "recorded SHA"):
                assets.prepare("linux-amd64", self.cache, self.output)
        api.assert_not_called()
        network.assert_not_called()

    def test_kubo_binary_checksum_mismatch_refused_before_execution(self):
        self.bin.write_bytes(b"different Kubo binary")
        with patch.object(assets.subprocess, "run") as command:
            with self.assertRaisesRegex(ValueError, "pinned capsule entrypoint"):
                self.prepare()
        command.assert_not_called()
        self.assertFalse(self.kubo.uploads)

    def test_wrong_api_repository_and_profile_refused(self):
        with patch.object(self.kubo, "repo_path", return_value=str(self.root / "another-repo")):
            with self.assertRaisesRegex(ValueError, "repository differs"):
                self.prepare()
        self.assertFalse(self.kubo.uploads)
        self.output = self.root / "second-artifacts"
        self.kubo.profile_error = True
        with self.assertRaisesRegex(assets.handoff.Refusal, "noncanonical profile"):
            self.prepare()
        self.assertFalse(self.kubo.uploads)

    def test_hash_add_mismatch_and_missing_recursive_pin_refuse_catalogue(self):
        self.kubo.retained_override = fixture_cid(b"wrong retained root")
        with self.assertRaisesRegex(ValueError, "differs from Kubo only-hash"):
            self.prepare()
        self.assertFalse(self.kubo.exports)
        self.assertFalse(list(self.output.glob(".model-closure-*")))
        self.output = self.root / "second-artifacts"
        self.kubo.retained_override, self.kubo.pin_verified = None, False
        with self.assertRaisesRegex(ValueError, "recursive retention pin"):
            self.prepare()
        self.assertFalse(self.kubo.exports)

    def test_export_stream_error_cleans_partial_car_and_staging(self):
        self.kubo.export_error = True
        with self.assertRaisesRegex(assets.handoff.Refusal, "export stream error"):
            self.prepare()
        handoff_dir = self.root / "artifacts-model-handoff"
        self.assertFalse(list(handoff_dir.iterdir()))
        self.assertFalse(list(self.output.glob(".model-closure-*")))

    def test_retention_and_export_disk_floors_run_before_their_writes(self):
        for index, before_export in enumerate((False, True)):
            self.output = self.root / ("floor-artifacts-" + str(index))
            handoff_dir = self.output.with_name(self.output.name + "-model-handoff")
            blocked = handoff_dir if before_export else self.repo
            self.kubo.uploads.clear()
            def gate(path, additional):
                if Path(path) == blocked:
                    raise ValueError("fixture 15% free space refusal")
            with patch.object(assets.upstream, "disk_gate", side_effect=gate):
                with self.assertRaisesRegex(ValueError, "15% free space"):
                    self.prepare()
            self.assertEqual(len(self.kubo.uploads), 2 if before_export else 1)
            self.assertFalse(self.kubo.exports)
            self.assertFalse(list(self.output.glob(".model-closure-*")))

    def test_handoff_inside_release_artifacts_is_refused(self):
        with self.assertRaisesRegex(ValueError, "outside release artifacts"):
            self.prepare(model_handoff_output=self.output / "nested-handoff")
        self.assertFalse(self.kubo.uploads)
        self.assertFalse((self.output / "nested-handoff").exists())

    def authoritative_handoff(self):
        result = self.prepare(model_publisher_did="did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c")
        source = self.root / "artifacts-model-handoff"
        self.output = self.root / "reuse-artifacts"
        return source, result

    def reuse(self, source, **options):
        arguments = {"model_handoff_input": source, "model_handoff_output": self.root / "reused-handoff",
                     "published_at": 123, "model_publisher_did": "did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c", **options}
        with (patch.object(assets.subprocess, "run") as execute,
              patch.object(assets.handoff, "KuboApi") as api,
              patch.object(assets, "add_model_directory") as add,
              patch.object(assets.handoff, "cmd_export") as export):
            result = assets.prepare("linux-amd64", self.cache, self.output, **arguments)
        execute.assert_not_called()
        api.assert_not_called()
        add.assert_not_called()
        export.assert_not_called()
        return result

    def test_authoritative_handoff_reuse_preserves_all_nine_bytes_without_kubo(self):
        source, original = self.authoritative_handoff()
        before = {path.name: path.read_bytes() for path in source.iterdir()}
        result = self.reuse(source)
        destination = self.root / "reused-handoff"
        self.assertEqual(len(before), 9)
        self.assertEqual(before, {path.name: path.read_bytes() for path in destination.iterdir()})
        self.assertEqual(before, {path.name: path.read_bytes() for path in source.iterdir()})
        self.assertEqual(result["model_retention"], original["model_retention"])
        self.assertEqual(result["model_catalog_unsigned"], original["model_catalog_unsigned"])
        self.assertEqual(json.loads((self.output / "upstream-input.json").read_bytes())["model_retention"], original["model_retention"])
        self.assertTrue(all(path.stat().st_mode & 0o077 == 0 for path in destination.iterdir()))
        # Another native platform retains the same models/evidence and writes
        # its own platform receipt. No model directory identity is recalculated.
        self.recipes[0]["platform"] = "darwin-arm64"
        self.write_inventory()
        second = self.root / "darwin-artifacts"
        with patch.object(assets.subprocess, "run") as execute:
            other = assets.prepare("darwin-arm64", self.cache, second, published_at=123,
                model_publisher_did="did:key:z6MkeTGwHmLmuCmgg4ABYhzWVh6ZX7hTwWt8gguAretUfc9c", model_handoff_input=source,
                model_handoff_output=self.root / "darwin-handoff")
        execute.assert_not_called()
        self.assertEqual(other["model_retention"], original["model_retention"])
        self.assertEqual(before, {path.name: path.read_bytes() for path in (self.root / "darwin-handoff").iterdir()})
        self.assertEqual(json.loads((second / "upstream-input.json").read_bytes())["platform"], "darwin-arm64")

    def test_reuse_excludes_kubo_and_requires_explicit_public_inputs_before_packaging(self):
        source, _ = self.authoritative_handoff()
        with patch.object(assets.upstream, "package") as package:
            for options in ({"model_kubo_bin": self.bin}, {"model_kubo_repo": self.repo},
                            {"published_at": True}, {"model_publisher_did": None}, {"model_handoff_output": None}):
                with self.subTest(options=options), self.assertRaises(ValueError):
                    self.reuse(source, **options)
            package.assert_not_called()

    def test_authoritative_handoff_refuses_extra_files_and_unprotected_or_linked_inputs(self):
        source, _ = self.authoritative_handoff()
        (source / "extra").write_bytes(b"unadvertised")
        with self.assertRaisesRegex(ValueError, "exactly nine"):
            self.reuse(source)
        (source / "extra").unlink()
        self.output = self.root / "second-artifacts"
        car = source / (sorted(assets.MODEL_COMPONENTS)[0] + ".car")
        car.chmod(0o644)
        with self.assertRaisesRegex(ValueError, "files must be protected"):
            self.reuse(source)
        car.chmod(0o600)
        self.output = self.root / "third-artifacts"
        outside = self.root / "retained-car"
        car.rename(outside)
        car.symlink_to(outside)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.reuse(source)
        car.unlink()
        outside.rename(car)
        assets.os.link(car, outside)
        self.output = self.root / "hardlink-artifacts"
        with self.assertRaisesRegex(ValueError, "single-link"):
            self.reuse(source)
        outside.unlink()
        source.chmod(0o755)
        with patch.object(assets.upstream, "package") as package, self.assertRaisesRegex(ValueError, "directory must be protected"):
            self.reuse(source)
        package.assert_not_called()

    def test_authoritative_handoff_missing_file_and_existing_output_are_refused(self):
        source, _ = self.authoritative_handoff()
        draft = source / "model-catalog.unsigned.json"
        original = draft.read_bytes()
        draft.unlink()
        with self.assertRaisesRegex(ValueError, "exactly nine"):
            self.reuse(source)
        self.assertFalse((self.root / "reused-handoff").exists())
        draft.write_bytes(original)
        draft.chmod(0o600)
        self.output = self.root / "second-artifacts"
        destination = self.root / "reused-handoff"
        destination.mkdir(mode=0o700)
        (destination / "owned-sentinel").write_bytes(b"preserve existing output")
        with self.assertRaisesRegex(ValueError, "output must be new"):
            self.reuse(source)
        self.assertEqual((destination / "owned-sentinel").read_bytes(), b"preserve existing output")

    def test_authoritative_catalogue_exact_timestamp_and_native_closure_are_required(self):
        source, _ = self.authoritative_handoff()
        with self.assertRaisesRegex(ValueError, "catalogue facts"):
            self.reuse(source, published_at=124)
        self.output = self.root / "second-artifacts"
        draft = source / "model-catalog.unsigned.json"
        payload = json.loads(draft.read_bytes())
        payload["entries"][0]["capsule_manifest"]["model_content"]["minimum_memory_mb"] = 512.0
        draft.write_bytes(assets.upstream.canonical(payload))
        with self.assertRaisesRegex(ValueError, "native model closure"):
            self.reuse(source)

    def test_authoritative_car_receipt_profile_and_payload_hash_refused(self):
        source, _ = self.authoritative_handoff()
        name = sorted(assets.MODEL_COMPONENTS)[0]
        receipt_path = source / (name + ".car.receipt.json")
        receipt = json.loads(receipt_path.read_bytes())
        receipt["kubo_version"] = "0.39.0"
        receipt_path.write_bytes(json.dumps(receipt).encode())
        with self.assertRaisesRegex(ValueError, "profile"):
            self.reuse(source)
        self.output = self.root / "second-artifacts"
        receipt["kubo_version"] = "0.40.1"
        receipt_path.write_bytes(json.dumps(receipt).encode())
        car = source / (name + ".car")
        data = car.read_bytes()
        car.write_bytes(b"X" + data[1:])
        with self.assertRaisesRegex(assets.handoff.Refusal, "CAR sha256"):
            self.reuse(source)

    def test_authoritative_reuse_floor_and_copy_failure_preserve_source_and_clean_output(self):
        source, _ = self.authoritative_handoff()
        before = {path.name: path.read_bytes() for path in source.iterdir()}
        destination = self.root / "reused-handoff"
        def gate(path, additional):
            if Path(path) == destination.parent:
                raise ValueError("fixture 15% free space refusal")
        with patch.object(assets.upstream, "disk_gate", side_effect=gate), self.assertRaisesRegex(ValueError, "15%"):
            self.reuse(source)
        self.assertFalse(destination.exists())
        self.assertFalse(list(self.root.glob(".reuse-model-handoff-*")))
        self.output = self.root / "second-artifacts"
        real_fsync, copies = assets.os.fsync, []
        def sync(fd):
            copies.append(fd)
            if len(copies) == 3:
                raise OSError("fixture copy fsync failure")
            return real_fsync(fd)
        with patch.object(assets.os, "fsync", side_effect=sync), self.assertRaisesRegex(OSError, "copy fsync"):
            self.reuse(source)
        self.assertFalse(destination.exists())
        self.assertFalse(list(self.root.glob(".reuse-model-handoff-*")))
        self.assertEqual(before, {path.name: path.read_bytes() for path in source.iterdir()})

    def rewrite_archive(self, receipt, transform):
        path = self.output / receipt["release_path"]
        with tarfile.open(path) as archive:
            records = [(member, archive.extractfile(member).read()) for member in archive]
        records = transform(records)
        with tarfile.open(path, "w:gz") as archive:
            for member, data in records:
                member.size = len(data)
                archive.addfile(member, io.BytesIO(data) if member.isreg() else None)
        receipt["size"] = path.stat().st_size
        receipt["checksum"] = "sha256:" + assets.upstream.digest(path)

    def test_exact_index_bytes_and_regular_file_closure_are_required(self):
        recipe, receipt = self.model_package()
        self.rewrite_archive(receipt, lambda records: [(member, data + b" " if member.name.endswith("_elastos_object.json") else data) for member, data in records])
        with tempfile.TemporaryDirectory(dir=self.root) as stage, self.assertRaisesRegex(ValueError, "path, size|hash|metadata bytes"):
            assets.extract_model(self.output, recipe, receipt, Path(stage))
        self.output = self.root / "second-artifacts"
        recipe, receipt = self.model_package()
        def link(records):
            member = records[0][0]
            member.type, member.linkname = tarfile.SYMTYPE, "../outside"
            return records
        self.rewrite_archive(receipt, link)
        with tempfile.TemporaryDirectory(dir=self.root) as stage, self.assertRaisesRegex(ValueError, "exact regular"):
            assets.extract_model(self.output, recipe, receipt, Path(stage))

    def test_extraction_disk_floor_and_modified_staged_bytes_refused(self):
        recipe, receipt = self.model_package()
        with tempfile.TemporaryDirectory(dir=self.root) as temporary:
            stage = Path(temporary)
            with patch.object(assets.upstream.shutil, "disk_usage", return_value=SimpleNamespace(total=1000, free=151)):
                with self.assertRaisesRegex(ValueError, "15% free"):
                    assets.extract_model(self.output, recipe, receipt, stage)
            self.assertFalse(list(stage.iterdir()))
            files, _ = assets.extract_model(self.output, recipe, receipt, stage)
            (stage / recipe["entrypoint"]).write_bytes(b"GGUFtampered")
            with self.assertRaisesRegex(ValueError, "staged model bytes changed"):
                assets.add_model_directory(self.kubo, stage, files, True)
            self.assertTrue(self.kubo.connections[-1].closed)

    def test_directory_response_error_bounds_and_root_identity_refused(self):
        recipe, receipt = self.model_package()
        with tempfile.TemporaryDirectory(dir=self.root) as temporary:
            stage = Path(temporary)
            files, _ = assets.extract_model(self.output, recipe, receipt, stage)
            cases = [(500, b"error", {}), (200, b"x" * (assets.handoff.MAX_CONTROL_BYTES + 1), {}),
                (200, b"", {"X-Stream-Error": "fixture stream failure"}),
                (200, json.dumps({"Name": "", "Hash": fixture_cid(b"raw", 0x55)}).encode(), {}),
                (200, b'{"Message":"fixture error"}', {}), (200, b"", {})]
            for status, data, trailers in cases:
                response = io.BytesIO(data)
                response.status, response.trailers = status, trailers
                self.kubo.response_override = response
                with self.subTest(status=status, data=data[:64]), self.assertRaises(ValueError):
                    assets.add_model_directory(self.kubo, stage, files, True)
                self.assertTrue(self.kubo.connections[-1].closed)


if __name__ == "__main__":
    unittest.main()
