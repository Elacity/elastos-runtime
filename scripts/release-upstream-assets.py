#!/usr/bin/env python3
"""Prepare checksum-pinned upstream capsules for a native release worker.

Opt-in model preparation uses a prepared build Kubo repository. It writes an
unsigned catalogue and streamed CARs beside the artifact directory, or into
--model-handoff-output. Those files and proposed retention descriptors belong
to the operator handoff; consumer manifests keep their existing shape.
--model-handoff-input reuses one authoritative nine-file export on other native
workers. Copies preserve its exact catalogue and factual export timestamps.
"""

import argparse
import base64
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import urllib.parse


SOURCE_ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("upstream_input", SOURCE_ROOT / "scripts/release-upstream-input.py")
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)
RECIPE_FILE = SOURCE_ROOT / "scripts/release-upstream-recipes.json"
handoff_spec = importlib.util.spec_from_file_location("model_package_handoff", SOURCE_ROOT / "scripts/model-package-handoff.py")
handoff = importlib.util.module_from_spec(handoff_spec)
sys.modules[handoff_spec.name] = handoff
handoff_spec.loader.exec_module(handoff)
MODEL_COMPONENTS = {"model-qwen3.5-0.8b", "model-qwen3.5-4b", "model-qwen3.5-9b", "model-bonsai-8b-q1"}
# Match directory_hash.rs. Only-hash and pin change together for retention.
MODEL_ADD_OPTIONS = {
    "wrap-with-directory": "true", "cid-version": "1", "hash": "sha2-256",
    "raw-leaves": "true", "chunker": "size-262144", "trickle": "false",
    "max-file-links": "174", "max-directory-links": "0", "max-hamt-fanout": "256",
    "inline": "false", "inline-limit": "32", "nocopy": "false", "fscache": "false",
    "preserve-mode": "false", "preserve-mtime": "false", "empty-dirs": "false",
    "progress": "false", "fast-provide-root": "false", "fast-provide-wait": "false",
}


def selected_recipes(platform):
    document = json.loads(RECIPE_FILE.read_text())
    if document.get("schema") != "elastos.release-upstream-recipes/v1":
        raise ValueError("unsupported upstream recipe inventory")
    selected = [recipe for recipe in document["recipes"] if recipe["platform"] in (platform, "*")]
    names = [recipe["component"] for recipe in selected]
    if len(names) != len(set(names)):
        raise ValueError("duplicate upstream component for the selected platform")
    return selected


def resolved_recipe(original, llama_arm64_bundle=None):
    recipe = copy.deepcopy(original)
    sources = [recipe["source"], *[item["source"] for item in recipe["license"]["files"]],
               *[item["source"] for item in recipe.get("notices", [])]]
    for source in sources:
        if "path" not in source:
            continue
        if source["path"] == "@llama-arm64-bundle":
            if llama_arm64_bundle is None:
                raise ValueError("qualified Linux ARM64 llama bundle is required")
            source["path"] = str(Path(llama_arm64_bundle).resolve(strict=True))
        else:
            relative = upstream.relative(source["path"])
            path = SOURCE_ROOT / relative
            upstream.regular(path)
            source["path"] = str(path.resolve(strict=True))
    return recipe


def directory_cid(value):
    if not isinstance(value, str) or not re.fullmatch(r"b[a-z2-7]{58}", value):
        raise ValueError("model root requires a canonical DAG-PB SHA-256 CIDv1")
    decoded = base64.b32decode(value[1:].upper() + "=" * (-len(value[1:]) % 8))
    if (decoded[:4] != b"\x01\x70\x12\x20" or len(decoded) != 36
            or "b" + base64.b32encode(decoded).decode().lower().rstrip("=") != value):
        raise ValueError("model root requires a canonical DAG-PB SHA-256 CIDv1")
    return value


def model_kubo(binary, repo, kubo_recipe, kubo_receipt):
    binary = upstream.regular(Path(binary).absolute())
    if not os.access(binary, os.X_OK):
        raise ValueError("prepared model Kubo binary must be executable")
    repo = Path(repo).absolute()
    if not repo.is_dir():
        raise ValueError("prepared model Kubo repository is required")
    upstream.directory(repo)
    if kubo_recipe["version"] != handoff.KUBO_VERSION:
        raise ValueError("model Kubo recipe version differs from the canonical profile")
    expected = next(item for item in kubo_receipt["object_manifest"]["files"]
                    if item["path"] == kubo_recipe["entrypoint"])
    if binary.stat().st_size != expected["size"] or upstream.digest(binary) != expected["sha256"]:
        raise ValueError("model Kubo binary differs from its pinned capsule entrypoint")
    version = subprocess.run([str(binary), "version", "--number"],
        env={**os.environ, "IPFS_PATH": str(repo)}, check=True, capture_output=True, text=True, timeout=30)
    if version.stdout.strip() != handoff.KUBO_VERSION:
        raise ValueError("model Kubo binary version differs from the canonical profile")
    api_file = upstream.regular(repo / "api")
    address = handoff.read_bounded(api_file, 256).decode().strip()
    match = re.fullmatch(r"/ip4/127\.0\.0\.1/tcp/([1-9][0-9]{0,4})", address)
    if not match or int(match[1]) > 65535:
        raise ValueError("prepared model Kubo API must use IPv4 loopback")
    kubo = handoff.KuboApi("http://127.0.0.1:" + match[1], 300)
    kubo.verify_profile()
    if Path(kubo.repo_path()).resolve() != repo.resolve():
        raise ValueError("model Kubo API repository differs from the prepared repository")
    return kubo


def extract_model(output, recipe, receipt, stage):
    capsule = receipt["capsule_manifest"]
    index = receipt["object_manifest"]
    expected_capsule = {"schema": "elastos.capsule/v1", "name": recipe["component"], "version": recipe["version"],
        "role": "content", "type": "data", "projections": ["content"], "entrypoint": recipe["entrypoint"],
        "model_content": recipe["model_content"]}
    if (capsule != expected_capsule
            or set(index) != {"schema", "kind", "files", "content_digest"}
            or index["schema"] != "elastos.content.object.manifest/v1" or index["kind"] != "capsule"):
        raise ValueError("model closure metadata differs from its pinned recipe")
    files = index["files"]
    if not isinstance(files, list) or not 1 <= len(files) <= 32:
        raise ValueError("model closure file bound differs from the Runtime contract")
    expected = {}
    value = hashlib.sha256()
    for item in files:
        if (not isinstance(item, dict) or set(item) != {"path", "sha256", "size"}
                or type(item["size"]) is not int or not 0 < item["size"] <= upstream.MAX_BYTES
                or not re.fullmatch(r"[0-9a-f]{64}", item["sha256"])
                or item["path"] in expected or item["path"] == "_elastos_object.json"):
            raise ValueError("invalid model closure file record")
        upstream.relative(item["path"])
        expected[item["path"]] = item
        for field in (item["path"], item["sha256"], str(item["size"])):
            value.update(field.encode() + b"\0")
    if list(expected) != sorted(expected) or index["content_digest"] != "sha256:" + value.hexdigest():
        raise ValueError("model closure index digest or ordering differs")
    capsule_bytes = upstream.canonical(capsule)[:-1]
    index_bytes = upstream.canonical(index)
    metadata = {"capsule.json": capsule_bytes, "_elastos_object.json": index_bytes}
    expected["_elastos_object.json"] = {"size": len(index_bytes), "sha256": hashlib.sha256(index_bytes).hexdigest()}
    total = sum(item["size"] for item in expected.values())
    if total > upstream.MAX_BYTES:
        raise ValueError("model closure exceeds its size bound")
    upstream.disk_gate(stage, total)
    archive_path = upstream.regular(output / upstream.relative(receipt["release_path"]))
    if (archive_path.stat().st_size != receipt["size"]
            or "sha256:" + upstream.digest(archive_path) != receipt["checksum"]):
        raise ValueError("model capsule archive differs from its build receipt")
    seen = set()
    with tarfile.open(archive_path, "r|gz") as archive:
        for member in archive:
            prefix = recipe["root"] + "/"
            if (member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE) or member.pax_headers
                    or not member.name.startswith(prefix)):
                raise ValueError("model capsule requires exact regular closure files")
            name = member.name[len(prefix):]
            if name not in expected or name in seen or member.size != expected[name]["size"]:
                raise ValueError("model archive path, size or duplicate differs from its closure")
            seen.add(name)
            destination = stage / name
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            digest = hashlib.sha256()
            with archive.extractfile(member) as source, destination.open("xb") as sink:
                os.fchmod(sink.fileno(), 0o600)
                for chunk in iter(lambda: source.read(upstream.CHUNK), b""):
                    digest.update(chunk)
                    sink.write(chunk)
            if digest.hexdigest() != expected[name]["sha256"]:
                raise ValueError("model archive file hash differs from its closure")
            if name in metadata and destination.read_bytes() != metadata[name]:
                raise ValueError("model archive metadata bytes differ from its closure")
    if seen != set(expected):
        raise ValueError("model archive has an incomplete closure")
    return expected, total


def add_model_directory(kubo, stage, files, only_hash):
    kubo.verify_profile()
    boundary = "elastos-model-build-" + os.urandom(12).hex()
    headers = {name: (f'--{boundary}\r\nContent-Disposition: form-data; name="file"; filename="{name}"\r\n'
                     "Content-Type: application/octet-stream\r\n\r\n").encode() for name in sorted(files)}
    tail = f"--{boundary}--\r\n".encode()
    length = sum(len(headers[name]) + files[name]["size"] + 2 for name in headers) + len(tail)
    query = {**MODEL_ADD_OPTIONS, "only-hash": str(only_hash).lower(), "pin": str(not only_hash).lower()}
    connection = kubo._connect()
    try:
        connection.putrequest("POST", "/api/v0/add?" + urllib.parse.urlencode(query))
        connection.putheader("Content-Type", "multipart/form-data; boundary=" + boundary)
        connection.putheader("Content-Length", str(length))
        connection.endheaders()
        for name in headers:
            connection.send(headers[name])
            value, size = hashlib.sha256(), 0
            with upstream.regular(stage / name).open("rb") as source:
                remaining = files[name]["size"]
                while remaining:
                    chunk = source.read(min(upstream.CHUNK, remaining))
                    if not chunk:
                        break
                    size += len(chunk)
                    remaining -= len(chunk)
                    value.update(chunk)
                    connection.send(chunk)
                if source.read(1):
                    raise ValueError("staged model bytes grew during Kubo add")
            if size != files[name]["size"] or value.hexdigest() != files[name]["sha256"]:
                raise ValueError("staged model bytes changed during Kubo add")
            connection.send(b"\r\n")
        connection.send(tail)
        response = connection.getresponse()
        body = response.read(handoff.MAX_CONTROL_BYTES + 1)
        if response.status != 200 or len(body) > handoff.MAX_CONTROL_BYTES or response.trailers.get("X-Stream-Error"):
            raise ValueError("model Kubo directory add failed or exceeded its response bound")
    finally:
        connection.close()
    root = None
    for line in filter(None, body.splitlines()):
        record = json.loads(line)
        if (not isinstance(record, dict) or set(record) - {"Name", "Hash", "Size"}
                or not isinstance(record.get("Name"), str) or not isinstance(record.get("Hash"), str)):
            raise ValueError("model Kubo directory add receipt is invalid")
        if record["Name"] == "":
            if root is not None:
                raise ValueError("model Kubo directory add has duplicate roots")
            root = directory_cid(record["Hash"])
    if root is None:
        raise ValueError("model Kubo directory add has no root")
    kubo.verify_profile()
    return root


def prepare_model_retention(output, handoff_output, models, kubo, repo, published_at, publisher_did):
    entries, retention = [], {}
    for recipe, receipt in models:
        with tempfile.TemporaryDirectory(prefix=".model-closure-", dir=output) as temporary:
            stage = Path(temporary)
            files, total = extract_model(output, recipe, receipt, stage)
            expected = add_model_directory(kubo, stage, files, True)
            upstream.disk_gate(repo, total * 2 + upstream.METADATA_BYTES)
            actual = add_model_directory(kubo, stage, files, False)
            if actual != expected:
                raise ValueError("model retained directory CID differs from Kubo only-hash")
            if any(entry["cid"] == actual for entry in entries):
                raise ValueError("model catalogue requires distinct directory roots")
            if not kubo.is_recursively_pinned(actual):
                raise ValueError("model directory requires a recursive retention pin")
            car = handoff_output / (recipe["component"] + ".car")
            receipt_path = Path(str(car) + ".receipt.json")
            if any(path.exists() or path.is_symlink() for path in (car, receipt_path, Path(str(car) + ".partial"))):
                raise ValueError("model CAR output already exists")
            upstream.disk_gate(handoff_output, total * 2 + upstream.METADATA_BYTES)
            try:
                handoff.cmd_export(SimpleNamespace(kubo_api=kubo.base_url, data_dir=None, timeout_seconds=300,
                                                  cid=actual, output=str(car)))
                car.chmod(0o600)
                receipt_path.chmod(0o600)
                upstream.regular(car)
                upstream.regular(receipt_path)
                verified = handoff.check_car(car, receipt_path, actual)
                retention[recipe["component"]] = {"package_cid": actual,
                    "car": {"release_path": car.name, "checksum": "sha256:" + verified.car_sha256, "size": verified.car_bytes},
                    "receipt": {"release_path": receipt_path.name, "checksum": "sha256:" + upstream.digest(receipt_path),
                                "size": receipt_path.stat().st_size}}
            except BaseException:
                car.unlink(missing_ok=True)
                receipt_path.unlink(missing_ok=True)
                raise
            entries.append({"cid": actual, "capsule_manifest": receipt["capsule_manifest"], "object_manifest": receipt["object_manifest"]})
    if len({item["cid"] for item in entries}) != 4:
        raise ValueError("model catalogue requires four distinct directory roots")
    draft = {"schema": "elastos.model.catalog/v1", "published_at": published_at, "expires_at": None, "entries": entries}
    draft_bytes = upstream.canonical(draft)
    if len(draft_bytes) > handoff.MAX_CATALOG_BYTES:
        raise ValueError("unsigned model catalogue exceeds the Runtime byte bound")
    draft_path = handoff_output / "model-catalog.unsigned.json"
    with draft_path.open("xb") as sink:
        os.fchmod(sink.fileno(), 0o600)
        sink.write(draft_bytes)
    record = {"release_path": draft_path.name, "checksum": "sha256:" + hashlib.sha256(draft_bytes).hexdigest(),
              "size": len(draft_bytes), "publisher_did": publisher_did}
    return retention, record


def public_model_publisher(value):
    alphabet = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
    if type(value) is not str or not re.fullmatch(r"did:key:z6Mk[1-9A-HJ-NP-Za-km-z]{44}", value):
        raise ValueError("model publisher requires a canonical public Ed25519 DID")
    number = 0
    for char in value[len("did:key:z"):]:
        number = number * 58 + alphabet.index(char)
    decoded = number.to_bytes((number.bit_length() + 7) // 8, "big")
    if len(decoded) != 34 or decoded[:2] != b"\xed\x01":
        raise ValueError("model publisher requires a canonical public Ed25519 DID")
    return value


def protected_handoff_input(path):
    path = Path(path).absolute()
    if not path.is_dir():
        raise ValueError("authoritative model handoff directory is required")
    for entry in (path, *path.parents):
        if entry.is_symlink():
            raise ValueError("authoritative model handoff path contains a symlink")
    metadata = path.stat()
    if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077:
        raise ValueError("authoritative model handoff directory must be protected and owned")
    return path


def handoff_file(path):
    upstream.regular(path)
    metadata = path.stat()
    if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077:
        raise ValueError("authoritative model handoff files must be protected and owned")
    return metadata


def handoff_stamp(metadata):
    return (metadata.st_dev, metadata.st_ino, metadata.st_size, metadata.st_mode,
            metadata.st_uid, metadata.st_nlink, metadata.st_mtime_ns, metadata.st_ctime_ns)


def reuse_model_retention(source, destination, models, published_at, publisher_did):
    """Copy one operator-selected Kubo export; preserve all nine original bytes."""
    source = protected_handoff_input(source)
    if source.resolve() == destination.resolve():
        raise ValueError("authoritative model handoff input and output must differ")
    expected_names = {"model-catalog.unsigned.json"}
    expected_names.update(name + suffix for name in MODEL_COMPONENTS for suffix in (".car", ".car.receipt.json"))
    if {path.name for path in source.iterdir()} != expected_names:
        raise ValueError("authoritative model handoff requires exactly nine files")
    if destination.exists() or destination.is_symlink():
        raise ValueError("model handoff reuse output must be new")
    parent = destination.parent
    if not parent.is_dir() or any(path.is_symlink() for path in (parent, *parent.parents)):
        raise ValueError("model handoff output requires an existing canonical parent")
    metadata = parent.stat()
    if metadata.st_uid != os.geteuid() or metadata.st_mode & 0o022:
        raise ValueError("model handoff output parent must be owned and protected")
    stamps = {name: handoff_file(source / name) for name in expected_names}
    draft_path = source / "model-catalog.unsigned.json"
    draft_bytes = handoff.read_bounded(draft_path, handoff.MAX_CATALOG_BYTES)
    draft = json.loads(draft_bytes)
    if (type(draft) is not dict or set(draft) != {"schema", "published_at", "expires_at", "entries"}
            or draft["schema"] != handoff.CATALOG_SCHEMA or type(draft["published_at"]) is not int
            or draft["published_at"] != published_at or draft["expires_at"] is not None
            or type(draft["entries"]) is not list or len(draft["entries"]) != 4
            or draft_bytes != upstream.canonical(draft)):
        raise ValueError("authoritative unsigned catalogue facts or canonical bytes differ")
    capsules = {recipe["component"]: receipt for recipe, receipt in models}
    seen, roots, retention = set(), set(), {}
    for entry in draft["entries"]:
        if type(entry) is not dict or set(entry) != {"cid", "capsule_manifest", "object_manifest"}:
            raise ValueError("authoritative catalogue entry fields refused")
        root = directory_cid(entry["cid"])
        capsule = entry["capsule_manifest"]
        name = capsule.get("name") if type(capsule) is dict else None
        if name not in capsules or name in seen or root in roots:
            raise ValueError("authoritative catalogue requires four distinct pinned models")
        for key in ("capsule_manifest", "object_manifest"):
            if upstream.canonical(entry[key]) != upstream.canonical(capsules[name][key]):
                raise ValueError("authoritative catalogue differs from the native model closure")
        seen.add(name)
        roots.add(root)
        car, receipt_path = source / (name + ".car"), source / (name + ".car.receipt.json")
        total = sum(item["size"] for item in entry["object_manifest"]["files"])
        if not 0 < stamps[car.name].st_size <= total * 2 + upstream.METADATA_BYTES:
            raise ValueError("authoritative model CAR exceeds its closure bound")
        verified = handoff.check_car(car, receipt_path, root)
        if verified.kubo_version != handoff.KUBO_VERSION or not 0 <= verified.exported_at <= handoff.MAX_BUDGET:
            raise ValueError("authoritative model export profile or timestamp refused")
        retention[name] = {"package_cid": root,
            "car": {"release_path": car.name, "checksum": "sha256:" + verified.car_sha256, "size": verified.car_bytes},
            "receipt": {"release_path": receipt_path.name, "checksum": "sha256:" + upstream.digest(receipt_path),
                        "size": stamps[receipt_path.name].st_size}}
    pins = {record[kind]["release_path"]: record[kind] for record in retention.values() for kind in ("car", "receipt")}
    unsigned = {"release_path": draft_path.name, "checksum": "sha256:" + hashlib.sha256(draft_bytes).hexdigest(),
                "size": len(draft_bytes), "publisher_did": publisher_did}
    pins[draft_path.name] = unsigned
    upstream.disk_gate(parent, sum(pin["size"] for pin in pins.values()) + upstream.METADATA_BYTES)
    with tempfile.TemporaryDirectory(prefix=".reuse-model-handoff-", dir=parent) as temporary:
        stage = Path(temporary)
        for name, pin in sorted(pins.items()):
            with (source / name).open("rb") as origin, (stage / name).open("xb") as target:
                before = os.fstat(origin.fileno())
                if handoff_stamp(before) != handoff_stamp(stamps[name]):
                    raise ValueError("authoritative model handoff changed before copy")
                os.fchmod(target.fileno(), 0o600)
                value, size = hashlib.sha256(), 0
                for chunk in iter(lambda: origin.read(upstream.CHUNK), b""):
                    size += len(chunk)
                    if size > pin["size"]:
                        raise ValueError("authoritative model handoff changed during copy")
                    target.write(chunk)
                    value.update(chunk)
                target.flush()
                os.fsync(target.fileno())
                if (handoff_stamp(os.fstat(origin.fileno())) != handoff_stamp(before)
                        or handoff_stamp(handoff_file(source / name)) != handoff_stamp(before)
                        or size != pin["size"] or "sha256:" + value.hexdigest() != pin["checksum"]):
                    raise ValueError("authoritative model handoff changed during copy")
        if ({path.name for path in source.iterdir()} != expected_names
                or any(handoff_stamp(handoff_file(source / name)) != handoff_stamp(stamps[name]) for name in expected_names)):
            raise ValueError("authoritative model handoff changed after admission")
        if destination.exists() or destination.is_symlink():
            raise ValueError("model handoff output appeared during copy")
        stage.rename(destination)
    return retention, unsigned


def prepare(platform, cache, output, llama_arm64_bundle=None, model_kubo_bin=None, model_kubo_repo=None,
            published_at=None, model_publisher_did=None, model_handoff_output=None, model_handoff_input=None):
    reuse = model_handoff_input is not None
    model_options = (model_kubo_bin, model_kubo_repo, published_at)
    enabled = any(value is not None for value in (*model_options, model_publisher_did, model_handoff_output, model_handoff_input))
    if (reuse and (model_kubo_bin is not None or model_kubo_repo is not None)):
        raise ValueError("model handoff reuse excludes Kubo build inputs")
    if enabled and (model_publisher_did is None or type(published_at) is not int or not 0 <= published_at <= 2**63 - 1
            or not reuse and any(value is None for value in model_options)
            or reuse and (model_publisher_did is None or model_handoff_output is None)):
        raise ValueError("model preparation requires a public timestamp and complete fresh or reuse inputs")
    if reuse:
        model_handoff_input = protected_handoff_input(model_handoff_input)
    if enabled:
        public_model_publisher(model_publisher_did)
    recipes = selected_recipes(platform)
    if enabled and {recipe["component"] for recipe in recipes if recipe.get("model_content")} != MODEL_COMPONENTS:
        raise ValueError("model preparation requires the four pinned release models")
    output = upstream.directory(output)
    external, receipts, resolved = {}, [], {}
    for original in recipes:
        recipe = resolved_recipe(original, llama_arm64_bundle)
        receipt = upstream.package(recipe, cache, output)
        fields = ("release_path", "checksum", "size", "extract_path", "install_path", "binary_path")
        info = {key: receipt[key] for key in fields if key in receipt}
        metadata = receipt["capsule_metadata"]
        external[recipe["component"]] = {
            "platforms": {recipe["platform"]: info},
            "capsule_metadata": {"role": "content", "type": "data", "install_path": metadata["install_path"],
                                 "platforms": {recipe["platform"]: metadata}},
        }
        receipt["recipe_sha256"] = hashlib.sha256(upstream.canonical(original)).hexdigest()
        receipts.append(receipt)
        resolved[recipe["component"]] = (recipe, receipt)
    result = {"external": external}
    if enabled:
        if "kubo" not in resolved:
            raise ValueError("model preparation requires the pinned Kubo capsule")
        models = [resolved[name] for name in sorted(MODEL_COMPONENTS)]
        handoff_output = Path(model_handoff_output).absolute() if model_handoff_output else output.with_name(output.name + "-model-handoff")
        if handoff_output.resolve() == output.resolve() or output.resolve() in handoff_output.resolve().parents:
            raise ValueError("model handoff directory must be outside release artifacts")
        if reuse:
            retention, draft = reuse_model_retention(model_handoff_input, handoff_output, models, published_at, model_publisher_did)
        else:
            upstream.directory(handoff_output)
            if handoff_output.stat().st_mode & 0o077:
                raise ValueError("model handoff output directory must be private")
            kubo = model_kubo(model_kubo_bin, model_kubo_repo, *resolved["kubo"])
            retention, draft = prepare_model_retention(output, handoff_output, models, kubo, Path(model_kubo_repo), published_at, model_publisher_did)
        # Key-free proposal for the operator. Consumer component descriptors
        # stay unchanged until the publication writer contract is approved.
        result["model_retention"] = retention
        result["model_catalog_unsigned"] = draft
    # This receipt belongs to native preparation, not the installed manifest.
    receipt_path = output / "upstream-input.json"
    with receipt_path.open("x") as stream:
        json.dump({"schema": "elastos.release-upstream-assets/v1", "platform": platform,
                   "recipes_sha256": hashlib.sha256(RECIPE_FILE.read_bytes()).hexdigest(),
                   "capsules": receipts, **({"model_catalog_unsigned": result["model_catalog_unsigned"],
                   "model_retention": retention} if enabled else {})}, stream, indent=2)
        stream.write("\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("linux-amd64", "linux-arm64", "darwin-arm64"), required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--llama-arm64-bundle", type=Path)
    parser.add_argument("--model-kubo-bin", type=Path, help="opt in with the pinned prepared Kubo binary")
    parser.add_argument("--model-kubo-repo", type=Path, help="protected build repo with a running loopback API")
    parser.add_argument("--published-at", type=int, help="public timestamp for the unsigned model catalogue")
    parser.add_argument("--model-publisher-did", help="public intended catalogue signer; this command signs nothing")
    parser.add_argument("--model-handoff-input", type=Path, help="reuse the exact nine protected files from an authoritative model export; excludes Kubo args")
    parser.add_argument("--model-handoff-output", type=Path, help="separate CAR/catalogue handoff directory; defaults beside output")
    args = parser.parse_args()
    print(json.dumps(prepare(args.platform, args.cache, args.output, args.llama_arm64_bundle,
        args.model_kubo_bin, args.model_kubo_repo, args.published_at, args.model_publisher_did, args.model_handoff_output, args.model_handoff_input)))


if __name__ == "__main__":
    main()
