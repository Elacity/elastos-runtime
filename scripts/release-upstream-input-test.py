#!/usr/bin/env python3
"""Inert build-input fixtures: no downloads, installs, signers or executables."""

import copy
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import tarfile
import tempfile
import unittest
from unittest.mock import patch
import urllib.error
import zipfile


spec = importlib.util.spec_from_file_location("upstream", Path(__file__).with_name("release-upstream-input.py"))
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)


class UpstreamTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.cache = self.root / "cache"
        self.output = self.root / "output"

    def source(self, name, data):
        path = self.root / name
        path.write_bytes(data)
        return {"path": str(path), "checksum": "sha256:" + hashlib.sha256(data).hexdigest(), "max_bytes": len(data)}

    def recipe(self, data=b"inert binary bytes"):
        return {"schema": upstream.SCHEMA, "component": "fixture", "platform": "linux-amd64",
                "version": "1.0.0", "source": self.source("payload", data), "format": "raw",
                "root": "fixture", "entrypoint": "binary", "extract_path": "fixture/binary",
                "install_path": "bin/fixture", "max_unpacked_bytes": 4096,
                "license": {"spdx_id": "MIT", "files": [{"name": "LICENSE", "source": self.source("license", b"fixture license notice\n")}]}}

    def package(self, recipe):
        return upstream.package(recipe, self.cache, self.output)

    def test_reproducible_capsule_preserves_payload_install_and_license(self):
        recipe = self.recipe()
        result = self.package(recipe)
        result2 = upstream.package(recipe, self.cache, self.root / "output2")
        self.assertEqual(result, result2)
        self.assertEqual(result["install_path"], "bin/fixture")
        self.assertEqual(result["extract_path"], "fixture/binary")
        self.assertEqual(result["capsule_metadata"]["extract_path"], "fixture")
        with tarfile.open(self.output / result["release_path"]) as archive:
            self.assertEqual(archive.extractfile("fixture/binary").read(), b"inert binary bytes")
            self.assertEqual(archive.extractfile("fixture/LICENSE").read(), b"fixture license notice\n")
            capsule = json.load(archive.extractfile("fixture/capsule.json"))
            self.assertEqual(capsule["role"], "content")
            self.assertEqual(capsule["type"], "data")
            provenance = json.load(archive.extractfile("fixture/PROVENANCE.json"))
            self.assertNotIn("path", provenance["upstream"])
            index = json.load(archive.extractfile("fixture/_elastos_object.json"))
            self.assertEqual(index, result["object_manifest"])
            self.assertNotIn("_elastos_object.json", [record["path"] for record in index["files"]])
            for record in index["files"]:
                data = archive.extractfile("fixture/" + record["path"]).read()
                self.assertEqual(record["sha256"], hashlib.sha256(data).hexdigest())
                self.assertEqual(record["size"], len(data))
            self.assertTrue(all(entry.mtime == 0 and entry.uid == 0 and entry.gid == 0 for entry in archive.getmembers()))

    def test_cache_fetches_pinned_network_input_once_and_checks_tampering(self):
        source = {"url": "https://software.example.test/input", "checksum": "sha256:" + hashlib.sha256(b"payload").hexdigest(), "max_bytes": 7}
        with patch.object(upstream, "response", return_value=io.BytesIO(b"payload")) as fetch:
            path = upstream.cached_input(source, self.cache)
            self.assertEqual(upstream.cached_input(source, self.cache), path)
            fetch.assert_called_once()
        path.write_bytes(b"tampered")
        with patch.object(upstream, "response") as fetch, self.assertRaisesRegex(ValueError, "cached upstream"):
            upstream.cached_input(source, self.cache)
        fetch.assert_not_called()

    def test_checksum_mismatch_and_size_bound_leave_no_cache_or_package(self):
        recipe = self.recipe()
        recipe["source"]["checksum"] = "sha256:" + "0" * 64
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.package(recipe)
        self.assertEqual(list(self.cache.iterdir()), [])
        self.assertEqual(list(self.output.iterdir()), [])
        recipe = self.recipe()
        recipe["source"]["max_bytes"] = 1
        with self.assertRaisesRegex(ValueError, "size bound"):
            self.package(recipe)
        self.assertEqual(list(self.cache.iterdir()), [])

    def test_url_only_missing_license_and_private_urls_refused_before_network(self):
        recipe = self.recipe()
        cases = []
        unpinned = copy.deepcopy(recipe)
        unpinned["source"] = {"url": "https://example.test/software", "max_bytes": 100}
        cases.append(unpinned)
        no_license = copy.deepcopy(recipe)
        no_license["license"]["files"] = []
        cases.append(no_license)
        for url in ("http://example.test/input", "https://127.0.0.1/input", "https://localhost/input", "file:///etc/passwd", "https://user:password@example.test/input"):
            unsafe = copy.deepcopy(recipe)
            unsafe["source"].pop("path")
            unsafe["source"]["url"] = url
            cases.append(unsafe)
        with patch.object(upstream, "response") as fetch:
            for case in cases:
                with self.subTest(source=case["source"]), self.assertRaises(ValueError):
                    self.package(case)
            fetch.assert_not_called()
        self.assertFalse(self.cache.exists())

    def test_disk_floor_refuses_fetch(self):
        source = self.recipe()["source"]
        with patch.object(upstream.shutil, "disk_usage", return_value=upstream.shutil._ntuple_diskusage(1000, 851, 149)), self.assertRaisesRegex(ValueError, "15%"):
            upstream.cached_input(source, self.cache)
        self.assertEqual(list(self.cache.iterdir()), [])

    def tar(self, members):
        target = io.BytesIO()
        with tarfile.open(fileobj=target, mode="w:gz") as archive:
            for name, kind, *values in members:
                entry = tarfile.TarInfo(name)
                if kind == "symlink":
                    entry.type, entry.linkname = tarfile.SYMTYPE, values[0] if values else "binary"
                    archive.addfile(entry)
                elif kind == "hardlink":
                    entry.type, entry.linkname = tarfile.LNKTYPE, values[0] if values else "fixture/binary"
                    archive.addfile(entry)
                elif kind in ("directory", "fifo"):
                    entry.type = tarfile.DIRTYPE if kind == "directory" else tarfile.FIFOTYPE
                    archive.addfile(entry)
                else:
                    data = values[0] if values else b"payload"
                    entry.size, entry.mode = len(data), values[1] if len(values) > 1 else 0o755
                    archive.addfile(entry, io.BytesIO(data))
        return target.getvalue()

    def test_unsafe_archives_refused(self):
        for entries in ([('../escape', 'file')], [('fixture/binary', 'file'), ('fixture/binary', 'file')],
                        [('fixture/binary', 'hardlink')], [('elsewhere/binary', 'file')],
                        [('fixture/binary', 'file'), ('fixture/binary/child', 'file')]):
            recipe = self.recipe(self.tar(entries))
            recipe["format"] = "tar.gz"
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                self.package(recipe)
            self.assertEqual(list(self.output.iterdir()), [])

    def test_combined_payload_and_injected_notice_paths_refuse_parent_conflicts_and_case_aliases(self):
        for conflict in ("LICENSE/child", "license", "CAPSULE.json"):
            recipe = self.recipe(self.tar([("fixture/binary", "file"), ("fixture/" + conflict, "file")]))
            recipe["format"] = "tar.gz"
            with self.subTest(conflict=conflict), patch.object(upstream.tempfile, "mkstemp", wraps=upstream.tempfile.mkstemp) as create:
                with self.assertRaisesRegex(ValueError, "closure"):
                    self.package(recipe)
                self.assertFalse(any(call.kwargs.get("prefix") == ".capsule-" for call in create.call_args_list))
                self.assertFalse(list(self.output.iterdir()))

    def test_archive_keeps_library_paths_and_executable_modes(self):
        recipe = self.recipe(self.tar([('fixture/binary', 'file'), ('fixture/lib/runtime.so', 'file')]))
        recipe["format"], recipe["extract_path"], recipe["binary_path"] = "tar.gz", "fixture", "binary"
        result = self.package(recipe)
        with tarfile.open(self.output / result["release_path"]) as archive:
            self.assertEqual(archive.extractfile("fixture/lib/runtime.so").read(), b"payload")
            self.assertEqual(archive.getmember("fixture/binary").mode, 0o755)

    def test_versioned_library_alias_chains_become_indexed_regular_files(self):
        elf = b"\x7fELF inert shared-library fixture"
        members = [('fixture/binary', 'file'),
                   ('fixture/lib/libllama.so', 'symlink', 'libllama.so.0'),
                   ('fixture/lib/libllama.so.0', 'symlink', 'libllama.so.0.0.0'),
                   ('fixture/lib/libllama.so.0.0.0', 'file', elf, 0o755),
                   ('fixture/lib/libllama-copy.so', 'hardlink', 'fixture/lib/libllama.so'),
                   ('fixture/lib/notice-copy', 'hardlink', 'fixture/notice'),
                   ('fixture/notice', 'file', b'library notice', 0o644),
                   ('fixture/notice-alias', 'symlink', './lib/../notice')]
        recipe = self.recipe(self.tar(members))
        recipe['format'], recipe['extract_path'], recipe['binary_path'] = 'tar.gz', 'fixture', 'binary'
        result = self.package(recipe)
        self.assertEqual(result, upstream.package(recipe, self.cache, self.root / 'output2'))
        self.assertEqual(upstream.digest(Path(recipe['source']['path'])), recipe['source']['checksum'][7:])
        cached = upstream.cached_input(recipe['source'], self.cache)
        self.assertEqual(cached.read_bytes(), Path(recipe['source']['path']).read_bytes())
        self.assertEqual(upstream.digest(cached), recipe['source']['checksum'][7:])
        with tarfile.open(self.output / result['release_path']) as archive:
            self.assertTrue(all(member.isfile() for member in archive.getmembers()))
            index = {record['path']: record for record in result['object_manifest']['files']}
            for name in ('libllama.so', 'libllama.so.0', 'libllama.so.0.0.0', 'libllama-copy.so'):
                path = 'fixture/lib/' + name
                self.assertEqual(archive.extractfile(path).read(), elf)
                self.assertEqual(archive.getmember(path).mode, 0o755)
                self.assertEqual(index['lib/' + name]['size'], len(elf))
                self.assertEqual(index['lib/' + name]['sha256'], hashlib.sha256(elf).hexdigest())
            for name in ('lib/notice-copy', 'notice-alias'):
                self.assertEqual(archive.getmember('fixture/' + name).mode, 0o644)
                self.assertEqual(archive.extractfile('fixture/' + name).read(), b'library notice')

    def test_unsafe_tar_links_refused_before_output(self):
        cases = [
            [('fixture/link', 'symlink', '/etc/passwd')],
            [('fixture/link', 'symlink', '../outside')],
            [('fixture/link', 'hardlink', 'elsewhere/file')],
            [('fixture/link', 'symlink', 'missing')],
            [('fixture/link', 'symlink', 'second'), ('fixture/second', 'symlink', 'link')],
            [('fixture/link', 'hardlink', 'fixture/second'), ('fixture/second', 'hardlink', 'fixture/link')],
            [('fixture/dir', 'directory'), ('fixture/link', 'symlink', 'dir')],
            [('fixture/dir/file', 'file'), ('fixture/link', 'hardlink', 'fixture/dir')],
            [('fixture/link', 'symlink', 'binary'), ('fixture/link/child', 'file')],
            [('fixture/link/child', 'file'), ('fixture/link', 'symlink', 'binary')],
            [('fixture/link', 'symlink', 'binary'), ('fixture/alias', 'symlink', 'link/child')],
            [('fixture/fifo', 'fifo'), ('fixture/link', 'symlink', 'fifo')],
            [('fixture/link', 'symlink', 'binary'), ('fixture/link', 'symlink', 'binary')],
        ]
        for entries in cases:
            recipe = self.recipe(self.tar([('fixture/binary', 'file'), *entries]))
            recipe['format'] = 'tar.gz'
            with self.subTest(entries=entries), self.assertRaises(ValueError):
                self.package(recipe)
            self.assertEqual(list(self.output.iterdir()), [])

    def test_tar_alias_copies_count_toward_unpacked_bound(self):
        recipe = self.recipe(self.tar([('fixture/binary', 'file'),
                                       ('fixture/link', 'symlink', 'binary'),
                                       ('fixture/copy', 'hardlink', 'fixture/link')]))
        recipe['format'], recipe['max_unpacked_bytes'] = 'tar.gz', 20
        with self.assertRaisesRegex(ValueError, 'including alias copies'):
            self.package(recipe)
        self.assertEqual(list(self.output.iterdir()), [])
        recipe['max_unpacked_bytes'] = 21
        result = self.package(recipe)
        self.assertEqual(sum(item['size'] for item in result['object_manifest']['files']
                             if item['path'] in ('binary', 'link', 'copy')), 21)

    def test_zip_link_refused(self):
        target = io.BytesIO()
        with zipfile.ZipFile(target, "w") as archive:
            entry = zipfile.ZipInfo("fixture/binary")
            entry.external_attr = 0o120777 << 16
            archive.writestr(entry, "target")
        recipe = self.recipe(target.getvalue())
        recipe["format"] = "zip"
        with self.assertRaisesRegex(ValueError, "ZIP links"):
            self.package(recipe)

    def test_cache_symlink_and_existing_output_preserved(self):
        recipe = self.recipe()
        self.cache.symlink_to(self.root)
        with self.assertRaisesRegex(ValueError, "symlink"):
            self.package(recipe)
        self.cache.unlink()
        result = self.package(recipe)
        before = (self.output / result["release_path"]).read_bytes()
        with self.assertRaisesRegex(ValueError, "already exists"):
            self.package(recipe)
        self.assertEqual((self.output / result["release_path"]).read_bytes(), before)

    def test_redirect_host_and_response_bounds(self):
        source = {"url": "https://upstream.example.test/input", "max_bytes": 10}
        redirect = urllib.error.HTTPError(source["url"], 302, "redirect", {"Location": "https://unlisted.example.test/input"}, io.BytesIO())
        with patch.object(upstream.urllib.request, "build_opener") as build:
            build.return_value.open.side_effect = redirect
            with self.assertRaisesRegex(ValueError, "allowlist"):
                upstream.response(source)
            self.assertEqual(build.return_value.open.call_count, 1)
        with patch.object(upstream.urllib.request, "build_opener") as build:
            build.return_value.open.return_value.status = 200
            build.return_value.open.return_value.headers = {"Content-Length": "11"}
            with self.assertRaisesRegex(ValueError, "size bound"):
                upstream.response(source)
            build.return_value.open.return_value.close.assert_called_once()

    def model_recipe(self):
        recipe = self.recipe(b"GGUFfixture weights")
        recipe["component"] = "model-fixture"
        recipe["entrypoint"] = "weights.gguf"
        recipe["extract_path"] = "fixture/weights.gguf"
        recipe["license"] = {"spdx_id": "Apache-2.0", "files": [
            {"name": "LICENSE", "source": self.source("model-license", b"fixture Apache notice\n")},
            {"name": "LICENSE.base", "source": self.source("base-license", b"fixture base license\n")}]}
        recipe["notices"] = [{"name": "PROVENANCE.md", "source": self.source("model-provenance", b"fixture publisher provenance\n")}]
        recipe["model_content"] = {"format": "gguf", "quantization": "Q4_K_M", "engine": "llama.cpp",
            "consumer_interface": "elastos.provider.model", "consumer_interface_version": "0.1.0",
            "minimum_memory_mb": 512, "license": {"spdx_id": "Apache-2.0", "path": "LICENSE"},
            "provenance": {"base_repository": "fixture/base", "base_revision": "a" * 40,
                "quantized_repository": "fixture/quantized", "quantized_revision": "b" * 40,
                "base_license": {"spdx_id": "Apache-2.0", "path": "LICENSE.base"}, "path": "PROVENANCE.md"}}
        return recipe

    def test_model_closure_carries_license_base_and_exact_provenance(self):
        recipe = self.model_recipe()
        result = self.package(recipe)
        self.assertEqual(result["capsule_manifest"]["model_content"], recipe["model_content"])
        files = {item["path"] for item in result["object_manifest"]["files"]}
        self.assertTrue({"weights.gguf", "LICENSE", "LICENSE.base", "PROVENANCE.md", "capsule.json"}.issubset(files))
        expected_capsule = json.dumps(result["capsule_manifest"], sort_keys=True,
            separators=(",", ":"), ensure_ascii=False).encode()
        with tarfile.open(self.output / result["release_path"]) as archive:
            self.assertEqual(archive.getmember("fixture/weights.gguf").mode, 0o644)
            self.assertEqual(archive.extractfile("fixture/capsule.json").read(), expected_capsule)
            self.assertEqual(json.load(archive.extractfile("fixture/_elastos_object.json")), result["object_manifest"])
            for item in result["object_manifest"]["files"]:
                payload = archive.extractfile("fixture/" + item["path"]).read()
                self.assertEqual(item["size"], len(payload))
                self.assertEqual(item["sha256"], hashlib.sha256(payload).hexdigest())
        capsule_record = next(item for item in result["object_manifest"]["files"] if item["path"] == "capsule.json")
        self.assertEqual(capsule_record["size"], len(expected_capsule))
        self.assertEqual(capsule_record["sha256"], hashlib.sha256(expected_capsule).hexdigest())
        value = hashlib.sha256()
        for item in result["object_manifest"]["files"]:
            for field in (item["path"], item["sha256"], str(item["size"])):
                value.update(field.encode() + b"\0")
        self.assertEqual(result["object_manifest"]["content_digest"], "sha256:" + value.hexdigest())

    def test_model_contract_missing_notice_revision_or_unsupported_quantization_refused(self):
        recipe = self.model_recipe()
        cases = []
        for pointer, value in (("quantization", "Q3_INVALID"), ("minimum_memory_mb", 0)):
            case = copy.deepcopy(recipe)
            case["model_content"][pointer] = value
            cases.append(case)
        case = copy.deepcopy(recipe)
        case["model_content"]["provenance"]["base_revision"] = "main"
        cases.append(case)
        case = copy.deepcopy(recipe)
        case["notices"] = []
        cases.append(case)
        with patch.object(upstream, "cached_input") as fetch:
            for case in cases:
                with self.assertRaises(ValueError):
                    self.package(case)
            fetch.assert_not_called()

    def test_bonsai_q1_contract_keeps_exact_weight_and_notice_closure(self):
        recipe = self.model_recipe()
        recipe["model_content"]["quantization"] = "Q1_0"
        result = self.package(recipe)
        self.assertEqual(result["capsule_manifest"]["model_content"]["quantization"], "Q1_0")
        weights = next(item for item in result["object_manifest"]["files"] if item["path"] == "weights.gguf")
        self.assertEqual(weights["sha256"], hashlib.sha256(b"GGUFfixture weights").hexdigest())

    def test_sha512_cache_and_local_path_independent_provenance(self):
        recipe = self.recipe()
        source = recipe["source"]
        source["checksum"] = "sha512:" + hashlib.sha512(Path(source["path"]).read_bytes()).hexdigest()
        first = self.package(recipe)
        replacement = self.root / "another-public-input-path"
        replacement.write_bytes(Path(source["path"]).read_bytes())
        source["path"] = str(replacement)
        second = upstream.package(recipe, self.cache, self.root / "second")
        self.assertEqual(first, second)

    def test_archive_unpacked_bound_and_missing_entrypoint_refused(self):
        recipe = self.recipe(self.tar([('fixture/binary', 'file'), ('fixture/lib/runtime.so', 'file')]))
        recipe["format"] = "tar.gz"
        recipe["max_unpacked_bytes"] = 7
        with self.assertRaisesRegex(ValueError, "unpacked size bound"):
            self.package(recipe)
        recipe["max_unpacked_bytes"] = 4096
        recipe["entrypoint"] = "missing"
        with self.assertRaisesRegex(ValueError, "entrypoint"):
            self.package(recipe)

    def test_network_bound_is_enforced_without_content_length(self):
        source = {"url": "https://example.test/input", "checksum": "sha256:" + hashlib.sha256(b"payload").hexdigest(), "max_bytes": 2}
        with patch.object(upstream, "response", return_value=io.BytesIO(b"payload")), self.assertRaisesRegex(ValueError, "size bound"):
            upstream.cached_input(source, self.cache)
        self.assertEqual(list(self.cache.iterdir()), [])


if __name__ == "__main__":
    unittest.main()
