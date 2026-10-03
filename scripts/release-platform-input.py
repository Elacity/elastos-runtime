#!/usr/bin/env python3
"""Record and verify unsigned native build inputs. This tool never publishes."""

import argparse
import base64
import copy
from datetime import datetime, timezone
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import platform as host_platform
import posixpath
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile


SCRIPT_ROOT = Path(__file__).resolve().parent
SOURCE_ROOT = SCRIPT_ROOT.parent
SCHEMA = "elastos.release-platform-input/v1"
MODEL_COMPONENTS = {"model-qwen3.5-0.8b", "model-qwen3.5-4b", "model-qwen3.5-9b", "model-bonsai-8b-q1"}
PLATFORMS = {
    "x86_64-linux": ("linux-amd64", "x86_64-unknown-linux-musl", 62),
    "aarch64-linux": ("linux-arm64", "aarch64-unknown-linux-musl", 183),
    "aarch64-darwin": ("darwin-arm64", "aarch64-apple-darwin", 0x100000C),
}
spec = importlib.util.spec_from_file_location(
    "component_integrity", SCRIPT_ROOT / "components-release-integrity-check.py")
integrity = importlib.util.module_from_spec(spec)
spec.loader.exec_module(integrity)


def run(*args):
    return subprocess.check_output(args, cwd=SOURCE_ROOT, text=True).strip()


def digest(path):
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def regular_file(root, relative):
    if (not isinstance(relative, str) or "\\" in relative
            or any(part in {"", ".", ".."} for part in relative.split("/"))):
        raise ValueError(f"unsafe artifact path: {relative!r}")
    path = root
    for part in relative.split("/"):
        path = path / part
        if path.is_symlink():
            raise ValueError(f"symlink in artifact path: {relative}")
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"artifact is not a regular file: {relative}")
    return path


def catalog_head_cid(data):
    return "b" + base64.b32encode(b"\x01\x55\x12\x20" + hashlib.sha256(data).digest()).decode("ascii").lower().rstrip("=")


def admit_model_catalog_artifact(manifest, artifact_root, referenced):
    pin = manifest.get("model_catalog")
    if pin is None:
        return
    if not isinstance(pin, dict) or not isinstance(pin.get("head_cid"), str) or not pin["head_cid"]:
        raise ValueError("model_catalog.head_cid is required")
    with regular_file(artifact_root, "model-catalog.json").open("rb") as catalog:
        data = catalog.read(128 * 1024 + 1)
    if len(data) > 128 * 1024:
        raise ValueError("model catalogue artifact exceeds its metadata bound")
    actual = catalog_head_cid(data)
    if actual != pin["head_cid"]:
        raise ValueError(f"model-catalog.json head {actual} does not match pin {pin['head_cid']}")
    referenced.add("model-catalog.json")
    retention = manifest.get("model_retention")
    if retention is not None:
        signer, handoff = model_tools()
        envelope = signer.parse_json(data)
        if envelope.get("signer_did") not in pin.get("publisher_dids", []):
            raise ValueError("catalogue publisher differs from trust pin")
        roots = admit_catalog_payload(envelope.get("payload"), signer)
        admit_model_budget(pin.get("local_use"), envelope["payload"])
        admit_retention(retention, roots, artifact_root, referenced, handoff)


def model_tools():
    """Load reviewed source helpers, independent of candidate artifact paths."""
    modules = []
    for name in ("release-signer", "model-package-handoff"):
        module_name = "release_input_" + name.replace("-", "_")
        if module_name not in sys.modules:
            definition = importlib.util.spec_from_file_location(module_name, SCRIPT_ROOT / (name + ".py"))
            module = importlib.util.module_from_spec(definition)
            sys.modules[module_name] = module
            definition.loader.exec_module(module)
        modules.append(sys.modules[module_name])
    return modules


def admit_catalog_payload(payload, signer):
    signer.require(type(payload) is dict and set(payload) == {"schema", "published_at", "expires_at", "entries"}
                   and payload["schema"] == "elastos.model.catalog/v1", "catalogue payload fields refused")
    now = int(datetime.now(timezone.utc).timestamp())
    published, expiry = payload["published_at"], payload["expires_at"]
    signer.require(type(published) is int and 0 <= published <= now
                   and (expiry is None or type(expiry) is int and expiry > now and expiry > published),
                   "catalogue publication or expiry refused")
    signer.require(type(payload["entries"]) is list and len(payload["entries"]) == 4,
                   "four catalogue entries required")
    roots = {}
    for entry in payload["entries"]:
        signer.require(type(entry) is dict and set(entry) == {"cid", "capsule_manifest", "object_manifest"},
                       "catalogue entry fields refused")
        cid, capsule, index = entry["cid"], entry["capsule_manifest"], entry["object_manifest"]
        codec, _ = signer.cid_info(cid)
        signer.require(cid.startswith("b") and codec == 0x70 and cid not in roots.values(),
                       "distinct canonical directory CIDs required")
        signer.require(type(capsule) is dict and type(index) is dict
                       and capsule.get("name") in MODEL_COMPONENTS and capsule["name"] not in roots,
                       "catalogue component inventory differs")
        signer.catalogue_closure(capsule, index)
        roots[capsule["name"]] = cid
    signer.require(set(roots) == MODEL_COMPONENTS, "four canonical models required")
    return roots


def retention_descriptor(value):
    if (type(value) is not dict or set(value) != {"release_path", "checksum", "size"}
            or type(value["release_path"]) is not str
            or not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]{0,239}", value["release_path"])
            or value["release_path"].endswith(".")
            or type(value["checksum"]) is not str or not re.fullmatch(r"sha256:[0-9a-f]{64}", value["checksum"])
            or type(value["size"]) is not int or not 0 < value["size"] <= 16 * 1024**3):
        raise ValueError("model retention descriptor refused")
    return value


def admit_model_budget(budget, payload):
    if budget is None:
        return
    if (type(budget) is not dict or set(budget) != {"max_cache_bytes", "max_model_memory_bytes"}
            or any(type(value) is not int or not 0 < value < 2**63 for value in budget.values())):
        raise ValueError("approved model local-use budget refused")
    for entry in payload["entries"]:
        total = sum(item["size"] for item in entry["object_manifest"]["files"])
        # Runtime also stages the bounded object index, which is outside its own
        # closure list. Charge its actual canonical bytes with serializer slack.
        total += len(json.dumps(entry["object_manifest"], sort_keys=True, separators=(",", ":")).encode()) + 1
        charge = 3 * total + 8 * 1024**2 + 3 * 65536
        memory = entry["capsule_manifest"]["model_content"]["minimum_memory_mb"] * 1024**2
        if charge > budget["max_cache_bytes"] or memory > budget["max_model_memory_bytes"]:
            raise ValueError("model closure exceeds approved local-use budget")


def admit_retention(retention, roots, root, referenced, handoff):
    if type(retention) is not dict or set(retention) != MODEL_COMPONENTS:
        raise ValueError("four model retention records required")
    names = set(referenced)
    for name, record in retention.items():
        if (type(record) is not dict or set(record) != {"package_cid", "car", "receipt"}
                or record["package_cid"] != roots[name]):
            raise ValueError("model retention catalogue root differs")
        paths = {}
        for kind in ("car", "receipt"):
            info = retention_descriptor(record[kind])
            relative = info["release_path"]
            if relative in names:
                raise ValueError("model retention artifact alias refused")
            names.add(relative)
            path = regular_file(root, relative)
            if path.stat().st_size != info["size"] or "sha256:" + digest(path) != info["checksum"]:
                raise ValueError("model retention artifact bytes differ")
            paths[kind] = path
            referenced.add(relative)
        try:
            receipt = handoff.check_car(str(paths["car"]), str(paths["receipt"]), record["package_cid"])
        except handoff.Refusal as exc:
            raise ValueError(str(exc)) from exc
        if receipt.kubo_version != handoff.KUBO_VERSION:
            raise ValueError("model retention Kubo version differs")


def public_catalog_verification(data, publisher_did, openssl, parent):
    """Verify public bytes with an explicit OpenSSL 3 executable; key access is absent."""
    signer, _ = model_tools()
    envelope = signer.parse_json(data)
    signer.check_did(publisher_did)
    signer.require(set(envelope) == {"payload", "signature", "signer_did"}
                   and envelope["signer_did"] == publisher_did, "catalogue public publisher differs")
    roots = admit_catalog_payload(envelope["payload"], signer)
    signature = envelope["signature"]
    signer.require(type(signature) is str and re.fullmatch(r"[0-9a-f]{128}", signature),
                   "Ed25519 catalogue signature required")
    executable = Path(openssl)
    signer.require(executable.is_absolute() and executable == executable.resolve(strict=True)
                   and os.access(executable, os.X_OK), "explicit qualified OpenSSL path required")
    for path in (*executable.parents, executable):
        metadata = path.lstat()
        signer.require(metadata.st_uid in (0, os.geteuid()) and not metadata.st_mode & 0o022,
                       "OpenSSL path protection refused")
    tool_sha = digest(executable)
    def execute(arguments, cwd):
        result = subprocess.run([str(executable), *arguments], cwd=cwd,
                                env={"OPENSSL_CONF": "/dev/null", "LANG": "C"},
                                stdin=subprocess.DEVNULL, capture_output=True, timeout=20)
        signer.require(result.returncode == 0 and len(result.stdout) <= 65536 and len(result.stderr) <= 65536,
                       "OpenSSL public verification failed")
        return result.stdout
    signer.require(execute(["version"], parent).startswith(b"OpenSSL 3."), "OpenSSL 3 required")
    number = 0
    for char in publisher_did[len("did:key:z"):]:
        number = number * 58 + signer.BASE58.index(char)
    raw = number.to_bytes(34, "big")[2:]
    with tempfile.TemporaryDirectory(prefix=".model-public-verify-", dir=parent) as temporary:
        scratch = Path(temporary)
        (scratch / "public.der").write_bytes(bytes.fromhex("302a300506032b6570032100") + raw)
        (scratch / "digest").write_bytes(hashlib.sha256(b"elastos.model.catalog.v1\0" + signer.json_bytes(envelope["payload"])).digest())
        (scratch / "signature").write_bytes(bytes.fromhex(signature))
        execute(["pkeyutl", "-provider", "default", "-verify", "-rawin", "-pubin", "-keyform", "DER",
                 "-inkey", "public.der", "-in", "digest", "-sigfile", "signature"], scratch)
    signer.require(digest(executable) == tool_sha, "OpenSSL executable changed")
    return envelope, roots, {"path": str(executable), "sha256": tool_sha}


def protected_model_directory(path, private=True):
    held = directory_descriptor(path)
    try:
        metadata = os.fstat(held)
        if metadata.st_uid != os.geteuid() or metadata.st_mode & (0o077 if private else 0o022):
            raise ValueError("model handoff requires a protected owned directory")
    finally:
        os.close(held)


def finalize_models(args):
    signer, handoff = model_tools()
    source = Path(os.path.abspath(args.input))
    protected_model_directory(source, private=False)
    original = verify(source)
    if "support_origin" in original or "model_finalization" in original:
        raise ValueError("model finalization requires an original native build input")
    record_bytes = signer.regular_bytes(regular_file(source, "model-handoff.json"), 128 * 1024)
    record = signer.parse_json(record_bytes)
    fields = {"schema", "scope", "source", "platform", "version", "handoff_directory", "upstream_input_sha256",
              "model_catalog_unsigned", "model_retention", "files"}
    if (set(record) != fields or record["schema"] != "elastos.release-model-handoff/v1"
            or record["scope"] != "key-free model preparation; signed catalogue finalization follows"
            or type(record["source"]) is not dict or record["source"].get("clean") is not True
            or record["source"] != {key: original["source"][key] for key in ("commit", "tree", "clean")}
            or record["platform"] != original["platform"] or record["version"] != original["version"]):
        raise ValueError("model handoff native source binding differs")
    model_root = Path(os.path.abspath(args.handoff))
    if model_root.parent != source.parent or record["handoff_directory"] != "../" + model_root.name:
        raise ValueError("model handoff must be the recorded sibling directory")
    protected_model_directory(model_root)
    upstream_bytes = signer.regular_bytes(regular_file(source, "upstream-input.json"), signer.MAX_JSON)
    upstream = signer.parse_json(upstream_bytes)
    if (hashlib.sha256(upstream_bytes).hexdigest() != record["upstream_input_sha256"]
            or any(upstream.get(key) != record[key] for key in ("model_catalog_unsigned", "model_retention"))):
        raise ValueError("model handoff upstream binding differs")
    unsigned = record["model_catalog_unsigned"]
    if type(unsigned) is not dict or set(unsigned) != {"release_path", "checksum", "size", "publisher_did"}:
        raise ValueError("unsigned catalogue descriptor refused")
    signer.check_did(args.publisher_did)
    if unsigned["publisher_did"] != args.publisher_did or unsigned["release_path"] != "model-catalog.unsigned.json":
        raise ValueError("model handoff public publisher differs")
    expected = {unsigned["release_path"]: retention_descriptor({key: unsigned[key] for key in ("release_path", "checksum", "size")})}
    if type(record["model_retention"]) is not dict or set(record["model_retention"]) != MODEL_COMPONENTS:
        raise ValueError("four model handoff retention records required")
    for item in record["model_retention"].values():
        if type(item) is not dict or set(item) != {"package_cid", "car", "receipt"}:
            raise ValueError("model retention record fields refused")
        for kind in ("car", "receipt"):
            descriptor = retention_descriptor(item[kind])
            if descriptor["release_path"] in expected:
                raise ValueError("model handoff artifact alias refused")
            expected[descriptor["release_path"]] = descriptor
    file_pins = {name: {key: info[key] for key in ("checksum", "size")} for name, info in expected.items()}
    if record["files"] != file_pins or {path.name for path in model_root.iterdir()} != set(expected):
        raise ValueError("model handoff file inventory differs")
    for name, pin in file_pins.items():
        path = regular_file(model_root, name)
        metadata = path.stat()
        if (metadata.st_nlink != 1 or metadata.st_uid != os.geteuid() or metadata.st_mode & 0o077
                or metadata.st_size != pin["size"] or "sha256:" + digest(path) != pin["checksum"]):
            raise ValueError("model handoff file protection or bytes differ")
    payload = signer.parse_json(signer.regular_bytes(model_root / unsigned["release_path"], 128 * 1024))
    signed_bytes = signer.regular_bytes(args.catalogue, 128 * 1024)
    destination = Path(os.path.abspath(args.output))
    parent = destination.parent
    protected_model_directory(parent, private=False)
    if (destination.exists() or destination.is_symlink() or destination.is_relative_to(source)
            or destination.is_relative_to(model_root)):
        raise ValueError("model finalization requires a new output outside its inputs")
    tool_path = Path(args.openssl)
    if any(tool_path.is_relative_to(path) for path in (source, model_root, destination)):
        raise ValueError("qualified OpenSSL must be outside candidate inputs")
    # Reserve the complete copy plus bounded new control documents before even
    # creating public-verification scratch. CARs stay streamed throughout.
    total = sum(info["size"] for info in original["files"].values()) + sum(info["size"] for info in expected.values())
    total += 8 * 1024**2 + len(signed_bytes)
    usage = shutil.disk_usage(parent)
    if (usage.free - total) * 100 < usage.total * 15:
        raise ValueError("model finalization requires 15 percent free after its complete copy")
    envelope, roots, tool = public_catalog_verification(signed_bytes, args.publisher_did, args.openssl, parent)
    if signer.json_bytes(payload) != signer.json_bytes(envelope["payload"]):
        raise ValueError("signed catalogue payload differs from unsigned preparation")
    capsules = {item.get("component"): item for item in upstream.get("capsules", [])
                if item.get("component") in MODEL_COMPONENTS}
    if set(capsules) != MODEL_COMPONENTS:
        raise ValueError("four native model capsule receipts required")
    for entry in payload["entries"]:
        capsule = capsules[entry["capsule_manifest"]["name"]]
        if any(signer.json_bytes(capsule.get(key)) != signer.json_bytes(entry[key])
               for key in ("capsule_manifest", "object_manifest")):
            raise ValueError("catalogue closure differs from native capsule receipt")
    admitted = {unsigned["release_path"]}
    admit_retention(record["model_retention"], roots, model_root, admitted, handoff)
    manifest = signer.parse_json(regular_file(source, "components.json").read_bytes())
    if "model_retention" in manifest:
        raise ValueError("native input already has model retention")
    template = signer.parse_json(regular_file(source, "components-template.json").read_bytes())
    budget = template.get("model_catalog", {}).get("local_use")
    admit_model_budget(budget, payload)
    manifest["model_catalog"] = {"head_cid": catalog_head_cid(signed_bytes), "publisher_dids": [args.publisher_did]}
    if budget is not None:
        manifest["model_catalog"]["local_use"] = budget
    manifest["model_retention"] = record["model_retention"]
    receipt_bytes = signer.regular_bytes(regular_file(source, "platform-input.json"), signer.MAX_JSON)
    components_bytes = signer.regular_bytes(regular_file(source, "components.json"), signer.MAX_JSON)
    with tempfile.TemporaryDirectory(prefix=".finalize-models-", dir=parent) as temporary:
        output = Path(temporary) / "input"
        output.mkdir(mode=0o700)
        for name, pin in original["files"].items():
            target = output / name
            target.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            shutil.copyfile(regular_file(source, name), target)
            target.chmod(0o700 if pin["executable"] else 0o600)
            if file_record(target) != pin:
                raise ValueError("native input changed during model finalization")
        for name in admitted - {unsigned["release_path"]}:
            target = output / "artifacts" / name
            if target.exists():
                raise ValueError("model retention collides with a native artifact")
            shutil.copyfile(regular_file(model_root, name), target)
            target.chmod(0o600)
            if file_record(target)["sha256"] != file_pins[name]["checksum"][7:] or target.stat().st_size != file_pins[name]["size"]:
                raise ValueError("model handoff changed during finalization")
        (output / "artifacts/model-catalog.json").write_bytes(signed_bytes)
        (output / "components.json").write_bytes(signer.json_bytes(manifest))
        (output / "model-native-input.json").write_bytes(receipt_bytes)
        (output / "model-native-components.json").write_bytes(components_bytes)
        if "artifacts/model-catalog.json" in original["files"]:
            shutil.copyfile(regular_file(source, "artifacts/model-catalog.json"), output / "model-native-catalog.json")
        receipt = copy.deepcopy(original)
        receipt["model_finalization"] = {"native_receipt_sha256": hashlib.sha256(receipt_bytes).hexdigest(),
            "publisher_did": args.publisher_did, "catalogue_sha256": hashlib.sha256(signed_bytes).hexdigest(),
            "signature_domain": "elastos.model.catalog.v1", "openssl": tool,
            "scope": "public signature and CAR receipt bytes verified; retention import and consumer activation require acceptance"}
        receipt["files"] = {str(path.relative_to(output)): file_record(path)
                            for path in output.rglob("*") if path.is_file()}
        (output / "platform-input.json").write_bytes(signer.json_bytes(receipt))
        verify(output)
        if verify(source) != original or regular_file(source, "platform-input.json").read_bytes() != receipt_bytes:
            raise ValueError("native input changed during model finalization")
        if destination.exists() or destination.is_symlink():
            raise ValueError("model finalization output already exists")
        output.rename(destination)
    return receipt


def verify_model_finalization(root, receipt):
    signer, _ = model_tools()
    proof = receipt["model_finalization"]
    if (type(proof) is not dict or set(proof) != {"native_receipt_sha256", "publisher_did", "catalogue_sha256",
            "signature_domain", "openssl", "scope"} or proof["signature_domain"] != "elastos.model.catalog.v1"):
        raise ValueError("model finalization proof fields refused")
    original_bytes = signer.regular_bytes(regular_file(root, "model-native-input.json"), signer.MAX_JSON)
    if hashlib.sha256(original_bytes).hexdigest() != proof["native_receipt_sha256"]:
        raise ValueError("model native provenance receipt differs")
    original = verify_receipt_header(signer.parse_json(original_bytes))
    if "support_origin" in original or "model_finalization" in original:
        raise ValueError("nested model finalization provenance refused")
    for key, value in original.items():
        if key != "files" and receipt.get(key) != value:
            raise ValueError("model finalization rewrote native build provenance")
    for name, pin in original["files"].items():
        retained = {"components.json": "model-native-components.json",
                    "artifacts/model-catalog.json": "model-native-catalog.json"}.get(name, name)
        if file_record(regular_file(root, retained)) != pin:
            raise ValueError("model finalization original artifact differs")
    data = signer.regular_bytes(regular_file(root, "artifacts/model-catalog.json"), 128 * 1024)
    envelope = signer.parse_json(data)
    signer.check_did(proof["publisher_did"])
    if (set(envelope) != {"payload", "signature", "signer_did"}
            or envelope["signer_did"] != proof["publisher_did"]
            or hashlib.sha256(data).hexdigest() != proof["catalogue_sha256"]):
        raise ValueError("model finalization catalogue differs")
    original_components = signer.parse_json(regular_file(root, "model-native-components.json").read_bytes())
    handoff_record = signer.parse_json(regular_file(root, "model-handoff.json").read_bytes())
    budget = signer.parse_json(regular_file(root, "components-template.json").read_bytes()).get("model_catalog", {}).get("local_use")
    original_components["model_catalog"] = {"head_cid": catalog_head_cid(data), "publisher_dids": [proof["publisher_did"]]}
    if budget is not None:
        original_components["model_catalog"]["local_use"] = budget
    original_components["model_retention"] = handoff_record["model_retention"]
    if signer.parse_json(regular_file(root, "components.json").read_bytes()) != original_components:
        raise ValueError("model finalization changed unrelated native components")


def file_record(path):
    return {"sha256": digest(path), "size": path.stat().st_size,
            "executable": bool(path.stat().st_mode & 0o111)}


def check_version(version):
    if not isinstance(version, str) or not version:
        raise ValueError("missing release version")
    result = subprocess.run(["bash", str(SCRIPT_ROOT / "check-versioning.sh"), version],
                            capture_output=True, text=True)
    if result.returncode:
        raise ValueError(result.stderr.strip())


def check_binary(path, platform):
    with path.open("rb") as source:
        header = source.read(64)
    check_native_header(header, path.stat().st_mode, platform, path.name)


def check_native_header(header, mode, platform, label):
    machine = PLATFORMS[platform][2]
    if platform.endswith("-linux"):
        valid = (len(header) == 64 and header[:7] == b"\x7fELF\x02\x01\x01"
                 and struct.unpack_from("<H", header, 16)[0] in (2, 3)
                 and struct.unpack_from("<H", header, 18)[0] == machine)
    else:
        valid = (len(header) >= 32 and header[:4] == b"\xcf\xfa\xed\xfe"
                 and struct.unpack_from("<I", header, 4)[0] == machine
                 and struct.unpack_from("<I", header, 12)[0] == 2)
    if not valid or not mode & 0o111:
        raise ValueError(f"{label}: expected executable for {platform}")


def check_media_tools_records(records, info, platform):
    recipe = SOURCE_ROOT / "scripts/media-tools-build.py"
    wrapper = recipe.with_name("build-media-tools.sh")
    spec = importlib.util.spec_from_file_location("media_recipe", recipe)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    sources = {name: {"url": url, "sha256": sha} for name, (url, sha) in module.SOURCES.items()}
    expected = {"schema": "elastos.media-tools-build/v1", "platform": PLATFORMS[platform][0],
                "sources": sources, "recipe_sha256": digest(recipe), "wrapper_sha256": digest(wrapper)}
    if not isinstance(info, dict) or any(info.get(k) != v for k, v in expected.items()):
        raise ValueError("media-tools build metadata differs from the reviewed recipe/platform")
    required = {"bin/ffmpeg", "bin/ffprobe", "BUILD.md", "sources/media-tools-build.py",
                "sources/build-media-tools.sh", "licenses/FFmpeg-COPYING.GPLv2", "licenses/x264-COPYING"}
    required.update("sources/" + name for name in sources)
    if platform.endswith("-linux"):
        required.add("licenses/musl-COPYRIGHT")
    if set(records) != required or info.get("files") != records:
        raise ValueError("media-tools source/binary inventory or hashes differ from build metadata")
    if not isinstance(info.get("compiler"), str) or not info["compiler"].strip():
        raise ValueError("media-tools compiler record is missing")
    if platform.endswith("-linux"):
        musl = info.get("musl")
        if (not isinstance(musl, dict) or not isinstance(musl.get("version"), str)
                or not musl["version"].strip()
                or musl.get("license_sha256") != records["licenses/musl-COPYRIGHT"]["sha256"]):
            raise ValueError("media-tools musl license metadata is missing or differs")
    for name, source in sources.items():
        if records["sources/" + name]["sha256"] != source["sha256"]:
            raise ValueError(f"media-tools source checksum mismatch: {name}")
    for name, expected_sha in (("sources/media-tools-build.py", expected["recipe_sha256"]),
                               ("sources/build-media-tools.sh", expected["wrapper_sha256"])):
        if records[name]["sha256"] != expected_sha:
            raise ValueError(f"media-tools recipe bytes differ: {name}")
    if any(record["size"] <= 0 for record in records.values()):
        raise ValueError("media-tools package contains an empty required file")


def check_archive(path, extract_path=None, provider=False, home_cli_platform=None,
                  media_platform=None, engine_platform=None):
    seen = set()
    regular = set()
    links = set()
    contract = None
    media_records = {}
    media_info = None
    engine_libraries = 0
    renderer = "home-cli/bin/home-cli"
    with tarfile.open(path, "r|gz") as archive:
        for entry in archive:
            name = entry.name.rstrip("/")
            if (not name or "\\" in name or PurePosixPath(name).is_absolute()
                    or any(p in {"", ".", ".."} for p in name.split("/"))
                    or name in seen):
                raise ValueError(f"{path.name}: unsafe or duplicate archive member {name!r}")
            if engine_platform is not None and name != extract_path and not name.startswith(extract_path + "/"):
                raise ValueError(f"{path.name}: ARM64 engine member escapes its archive root: {name}")
            if media_platform is not None and (not name.startswith("media-tools/") and name != "media-tools"
                                               or not (entry.isfile() or entry.isdir())):
                raise ValueError(f"{path.name}: media-tools requires regular files within its archive root")
            if any(str(parent) in links or str(parent) in regular for parent in PurePosixPath(name).parents):
                raise ValueError(f"{path.name}: archive member beneath a file or link: {name}")
            if entry.issym():
                target = entry.linkname
                resolved = posixpath.normpath(posixpath.join(posixpath.dirname(name), target))
                if ("\\" in target or target.startswith("/") or resolved == ".."
                        or resolved.startswith("../") or resolved.split("/")[0] != name.split("/")[0]):
                    raise ValueError(f"{path.name}: archive link escapes capsule: {name}")
                links.add(name)
                if any(other.startswith(name + "/") for other in seen):
                    raise ValueError(f"{path.name}: link replaces archive parent: {name}")
            elif not (entry.isfile() or entry.isdir()):
                raise ValueError(f"{path.name}: unsupported archive entry type: {name}")
            if entry.isfile():
                if any(other.startswith(name + "/") for other in seen):
                    raise ValueError(f"{path.name}: file replaces archive parent: {name}")
                regular.add(name)
            if media_platform is not None and entry.isfile():
                stream = archive.extractfile(entry)
                if name == "media-tools/build-info.json":
                    if entry.size > 1024 * 1024:
                        raise ValueError("media-tools build metadata exceeds its bound")
                    media_info = json.load(stream)
                else:
                    header = stream.read(64)
                    if name in {"media-tools/bin/ffmpeg", "media-tools/bin/ffprobe"}:
                        check_native_header(header, entry.mode, media_platform, name)
                    file_hash = hashlib.sha256(header)
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        file_hash.update(chunk)
                    media_records[name.removeprefix("media-tools/")] = {"sha256": file_hash.hexdigest(), "size": entry.size}
            if home_cli_platform is not None and name == renderer:
                if not entry.isfile():
                    raise ValueError(f"{path.name}: Home CLI renderer must be a regular file")
                check_native_header(archive.extractfile(entry).read(64), entry.mode,
                                    home_cli_platform, renderer)
            if engine_platform is not None and entry.isfile() and (
                    name == f"{extract_path}/llama-server" or ".so" in PurePosixPath(name).name):
                check_native_header(archive.extractfile(entry).read(64), entry.mode,
                                    engine_platform, name)
                if ".so" in PurePosixPath(name).name:
                    engine_libraries += 1
            if provider and name == f"{extract_path}/capsule.json":
                if not entry.isfile() or entry.size > 1024 * 1024:
                    raise ValueError(f"{path.name}: invalid provider capsule manifest")
                contract = json.load(archive.extractfile(entry))
            seen.add(name)
    if home_cli_platform is not None and (extract_path != "home-cli" or renderer not in regular):
        raise ValueError(f"{path.name}: Home CLI native renderer is missing")
    if engine_platform is not None and f"{extract_path}/llama-server" not in regular:
        raise ValueError(f"{path.name}: ARM64 llama-server executable is missing")
    if engine_platform is not None and engine_libraries == 0:
        raise ValueError(f"{path.name}: ARM64 llama-server libraries are missing")
    if media_platform is not None:
        if extract_path != "media-tools":
            raise ValueError("media-tools extraction root differs from its contract")
        check_media_tools_records(media_records, media_info, media_platform)
    if not seen:
        raise ValueError(f"{path.name}: empty app archive")
    if extract_path is not None:
        if (not isinstance(extract_path, str) or not extract_path or "\\" in extract_path
                or any(p in {"", ".", ".."} for p in extract_path.split("/"))
                or extract_path in links
                or not any(n == extract_path or n.startswith(extract_path + "/") for n in seen)):
            raise ValueError(f"{path.name}: extraction root is missing or unsafe: {extract_path}")
    if provider:
        if not isinstance(contract, dict) or contract.get("role") != "provider":
            raise ValueError(f"{path.name}: provider capsule contract is missing")
        if contract.get("name") != extract_path:
            raise ValueError(f"{path.name}: provider capsule name differs from its component")
        icon = contract.get("icon")
        if (not isinstance(icon, str) or not icon or "\\" in icon
                or any(p in {"", ".", ".."} for p in icon.split("/"))):
            raise ValueError(f"{path.name}: invalid provider icon directory")
        for size in integrity.PROVIDER_ICON_SIZES:
            name = f"{extract_path}/{icon}/icon-{size}.png"
            if name not in regular:
                raise ValueError(f"{path.name}: missing provider icon {size}")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode() + b"\n"


def public_upstream_recipe(recipe):
    value = copy.deepcopy(recipe)
    for source in [value["source"], *[item["source"] for item in value["license"]["files"]],
                   *[item["source"] for item in value.get("notices", [])]]:
        source.pop("path", None)
    return value


def check_upstream_archive(path, recipe, receipt, platform):
    root = recipe["root"]
    records, metadata, headers, seen = {}, {}, {}, set()
    metadata_names = {"capsule.json", "PROVENANCE.json", "_elastos_object.json"}
    notices = {item["name"]: item for item in [*recipe["license"]["files"], *recipe.get("notices", [])]}
    total = 0
    with tarfile.open(path, "r|gz") as archive:
        for member in archive:
            name = member.name
            if (not member.isfile() or member.pax_headers or "\\" in name or name in seen
                    or not name.startswith(root + "/") or any(part in {"", ".", ".."} for part in name.split("/"))):
                raise ValueError("upstream capsule contains unsafe, duplicate or nonregular members")
            seen.add(name)
            short = name[len(root) + 1:]
            total += member.size
            if member.size < 0 or total > recipe["max_unpacked_bytes"] + 64 * 1024**2 or len(seen) > 4131:
                raise ValueError("upstream capsule exceeds its reviewed unpacked bound")
            stream = archive.extractfile(member)
            header = stream.read(64)
            value, license_hash = hashlib.sha256(header), None
            payload_hash = None
            if recipe["format"] == "raw" and short == recipe["entrypoint"]:
                payload_algorithm, payload_expected = recipe["source"]["checksum"].split(":", 1)
                payload_hash = hashlib.new(payload_algorithm, header)
            if short in notices:
                algorithm, expected = notices[short]["source"]["checksum"].split(":", 1)
                license_hash = hashlib.new(algorithm, header)
            captured = bytearray(header) if short in metadata_names else None
            if captured is not None and member.size > 1024**2:
                raise ValueError("upstream capsule metadata exceeds its bound")
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                value.update(chunk)
                if payload_hash is not None:
                    payload_hash.update(chunk)
                if license_hash is not None:
                    license_hash.update(chunk)
                if captured is not None:
                    captured.extend(chunk)
            if payload_hash is not None:
                if payload_hash.hexdigest() != payload_expected:
                    raise ValueError("upstream raw payload differs from its original source checksum")
            if license_hash is not None and license_hash.hexdigest() != expected:
                raise ValueError("upstream capsule licence or provenance notice differs from its source pin")
            if captured is not None:
                metadata[short] = json.loads(captured)
            headers[short] = (header, member.mode)
            if short != "_elastos_object.json":
                records[short] = {"path": short, "sha256": value.hexdigest(), "size": member.size}
    if not metadata_names <= set(metadata) or not set(notices) <= set(records):
        raise ValueError("upstream capsule metadata, licence or notice is missing")
    expected_capsule = {"schema": "elastos.capsule/v1", "name": recipe["component"],
        "version": recipe["version"], "role": "content", "type": "data", "projections": ["content"],
        "entrypoint": recipe["entrypoint"]}
    if "model_content" in recipe:
        expected_capsule["model_content"] = recipe["model_content"]
    if metadata["capsule.json"] != expected_capsule or receipt.get("capsule_manifest") != expected_capsule:
        raise ValueError("upstream content capsule contract differs from its recipe")
    source_record = lambda source: {key: value for key, value in source.items() if key != "path"}
    expected_provenance = {"schema": "elastos.release-upstream-input/v1", "component": recipe["component"],
        "platform": recipe["platform"], "license": recipe["license"]["spdx_id"],
        "recipe_sha256": hashlib.sha256(canonical(public_upstream_recipe(recipe))).hexdigest(),
        "upstream": source_record(recipe["source"]),
        "notices": [{"name": item["name"], "source": source_record(item["source"])}
                    for item in [*recipe["license"]["files"], *recipe.get("notices", [])]]}
    if metadata["PROVENANCE.json"] != expected_provenance:
        raise ValueError("upstream capsule provenance differs from its reviewed recipe")
    ordered = [records[name] for name in sorted(records)]
    closure = hashlib.sha256()
    for record in ordered:
        for field in (record["path"], record["sha256"], str(record["size"])):
            closure.update(field.encode() + b"\0")
    expected_index = {"schema": "elastos.content.object.manifest/v1", "kind": "capsule", "files": ordered,
                      "content_digest": "sha256:" + closure.hexdigest()}
    if metadata["_elastos_object.json"] != expected_index or receipt.get("object_manifest") != expected_index:
        raise ValueError("upstream capsule object closure differs from its actual files")
    entrypoint = recipe["entrypoint"]
    if entrypoint not in headers:
        raise ValueError("upstream capsule entrypoint is missing")
    header, mode = headers[entrypoint]
    if recipe.get("model_content"):
        if header[:4] != b"GGUF":
            raise ValueError("upstream model payload has no GGUF header")
    else:
        check_native_header(header, mode, platform, entrypoint)
        if recipe["component"] == "llama-server" and platform == "aarch64-linux":
            libraries = [name for name in headers if ".so" in PurePosixPath(name).name]
            if not libraries:
                raise ValueError("upstream ARM64 llama-server libraries are missing")
            for name in libraries:
                check_native_header(*headers[name], platform, name)


def admit_upstream_inputs(root, platform, manifest, template):
    setup = PLATFORMS[platform][0]
    inventory_path = root / "upstream-recipes.json"
    receipt_path = root / "upstream-input.json"
    source_inventory = SOURCE_ROOT / "scripts/release-upstream-recipes.json"
    if not inventory_path.exists():
        if source_inventory.exists():
            source = json.loads(source_inventory.read_bytes())
            selected = {r["component"] for r in source["recipes"] if r["platform"] in (setup, "*")}
            if selected & set(template["external"]):
                raise ValueError("upstream dependencies require retained recipe and input receipts")
        if receipt_path.exists():
            raise ValueError("upstream input receipt lacks retained recipes")
        return {}
    inventory = json.loads(regular_file(root, "upstream-recipes.json").read_bytes())
    document = json.loads(regular_file(root, "upstream-input.json").read_bytes())
    if (inventory.get("schema") != "elastos.release-upstream-recipes/v1"
            or document.get("schema") != "elastos.release-upstream-assets/v1" or document.get("platform") != setup
            or document.get("recipes_sha256") != digest(inventory_path)):
        raise ValueError("upstream recipe inventory or platform binding differs")
    recipes = [r for r in inventory["recipes"] if r["platform"] in (setup, "*") and r["component"] in template["external"]]
    by_name = {r["component"]: r for r in recipes}
    receipts = document.get("capsules", [])
    if (len(by_name) != len(recipes) or not isinstance(receipts, list)
            or len(receipts) != len(by_name) or {r.get("component") for r in receipts} != set(by_name)):
        raise ValueError("upstream component inventory differs from selected recipes")
    for receipt in receipts:
        recipe = by_name[receipt["component"]]
        if (receipt.get("schema") != "elastos.release-upstream-input/v1"
                or receipt.get("platform") != recipe["platform"]
                or receipt.get("recipe_sha256") != hashlib.sha256(canonical(recipe)).hexdigest()):
            raise ValueError("upstream input recipe/source binding differs")
        component = manifest["external"][recipe["component"]]
        _, info = integrity.resolve_platform_info(component, setup)
        fields = ("release_path", "checksum", "size", "extract_path", "install_path", "binary_path")
        if info != {key: receipt[key] for key in fields if key in receipt}:
            raise ValueError("upstream dependency descriptor differs from its build receipt")
        for key in ("extract_path", "install_path", "binary_path"):
            if receipt.get(key) != recipe.get(key):
                raise ValueError("upstream extraction/install contract differs from its recipe")
        metadata = component.get("capsule_metadata", {})
        _, metadata_info = integrity.resolve_platform_info(metadata, setup)
        if (metadata.get("role") != "content" or metadata.get("type") != "data"
                or not isinstance(metadata_info, dict) or metadata_info != receipt.get("capsule_metadata")
                or metadata_info.get("extract_path") != recipe["root"]
                or metadata_info.get("install_path") != "capsules/" + recipe["component"]):
            raise ValueError("upstream content metadata differs from its recipe/receipt")
        check_upstream_archive(regular_file(root / "artifacts", info["release_path"]), recipe, receipt, platform)
    return by_name


def check_contents(root, platform, omissions):
    setup_platform = PLATFORMS[platform][0]
    manifest = json.loads(regular_file(root, "components.json").read_text())
    template = json.loads(regular_file(root, "components-template.json").read_text())
    if manifest.get("schema") != "elastos.components/v1" or manifest.get("capsules") != {}:
        raise ValueError("native preparation requires a v1 manifest without generic VM capsules")
    if manifest.get("profiles") != template.get("profiles"):
        raise ValueError("prepared profiles differ from source template")
    if set(manifest.get("external", {})) != set(template.get("external", {})):
        raise ValueError("prepared component inventory differs from source template")
    if (not isinstance(omissions, list) or any(not isinstance(n, str) for n in omissions)
            or len(set(omissions)) != len(omissions)):
        raise ValueError("platform omissions must be unique component names")
    for name in omissions:
        component = template["external"].get(name)
        if component is None or integrity.resolve_platform_info(component, setup_platform)[1] is not None:
            raise ValueError(f"{name}: omission is not platform-absent in source template")
    errors = integrity.audit_manifest(manifest, [setup_platform])
    errors += integrity.audit_release_artifacts(manifest, [setup_platform], root / "artifacts")
    if errors:
        raise ValueError("; ".join(errors))
    upstream_recipes = admit_upstream_inputs(root, platform, manifest, template)
    referenced = {f"elastos-{platform}"}
    for name, component in manifest["external"].items():
        contract = lambda value: {k: v for k, v in value.items() if k not in ("platforms", "capsule_metadata")}
        if contract(component) != contract(template["external"][name]):
            raise ValueError(f"{name}: component contract differs from source template")
        original_component = template["external"][name]
        _, original_info = integrity.resolve_platform_info(original_component, setup_platform)
        selected_key, prepared_info = integrity.resolve_platform_info(component, setup_platform)
        if original_info is not None and prepared_info is None:
            raise ValueError(f"{name}: prepared platform is missing")
        if original_info is None and prepared_info is not None:
            raise ValueError(f"{name}: preparation added a platform absent from source")
        for key, value in component.get("platforms", {}).items():
            if key != selected_key and original_component.get("platforms", {}).get(key) != value:
                raise ValueError(f"{name}: unselected platform contract changed: {key}")
        if original_info is not None:
            if original_info.get("release_path") and not prepared_info.get("release_path"):
                raise ValueError(f"{name}: source-local component needs a local artifact")
            if name not in upstream_recipes and not original_info.get("release_path") and original_info.get("url") and prepared_info != original_info:
                raise ValueError(f"{name}: external dependency differs from pinned source template")
        provider_runtime = component.get("provider_runtime")
        if (original_info is not None and isinstance(provider_runtime, dict)
                and provider_runtime.get("runtime_only") is not True):
            metadata = component.get("capsule_metadata")
            if not isinstance(metadata, dict) or integrity.resolve_platform_info(metadata, setup_platform)[1] is None:
                raise ValueError(f"{name}: provider capsule metadata is missing")
        entries = [component]
        if isinstance(component.get("capsule_metadata"), dict):
            entries.append(component["capsule_metadata"])
        for entry in entries:
            selected_key, info = integrity.resolve_platform_info(entry, setup_platform)
            is_metadata = entry is not component
            is_provider_metadata = is_metadata and name not in upstream_recipes
            if is_provider_metadata:
                if info is None or not info.get("release_path") or info.get("extract_path") != name:
                    raise ValueError(f"{name}: provider metadata needs a local archive rooted at its capsule name")
                if set(entry.get("platforms", {})) != {selected_key}:
                    raise ValueError(f"{name}: provider metadata includes an unselected platform")
            if info is None or not info.get("release_path"):
                continue
            if any(key in info for key in ("cid", "url", "strategy")):
                raise ValueError(f"{name}: prepared local descriptor contains a transport or build strategy")
            relative = info["release_path"]
            referenced.add(relative)
            path = regular_file(root / "artifacts", relative)
            expected_install = f"capsules/{name}" if entry is not component else None
            if expected_install is None:
                _, original = integrity.resolve_platform_info(template["external"][name], setup_platform)
                expected_install = (original or {}).get("install_path", component.get("install_path"))
            if info.get("install_path", entry.get("install_path")) != expected_install:
                raise ValueError(f"{name}: prepared install path differs from source contract")
            if name in upstream_recipes:
                continue  # Complete archive admission above includes the extracted native payload.
            if info.get("extract_path"):
                check_archive(path, info["extract_path"], provider=is_provider_metadata,
                              home_cli_platform=platform if name == "home-cli" and not is_provider_metadata else None,
                              media_platform=platform if name == "media-tools" else None,
                              engine_platform=platform if name == "llama-server" and platform == "aarch64-linux" else None)
            elif info.get("install_path", entry.get("install_path", "")).startswith("bin/"):
                check_binary(path, platform)
            elif expected_install and expected_install.startswith("capsules/"):
                raise ValueError(f"{name}: capsule artifact needs an extraction path")
    check_binary(regular_file(root / "artifacts", f"elastos-{platform}"), platform)
    actual = {str(path.relative_to(root / "artifacts"))
              for path in (root / "artifacts").rglob("*") if not path.is_dir() or path.is_symlink()}
    admit_model_catalog_artifact(manifest, root / "artifacts", referenced)
    if actual != referenced:
        raise ValueError(f"artifact inventory mismatch: {sorted(actual ^ referenced)}")
    return manifest, template


def source_identity(commit, tree):
    if not re.fullmatch(r"[0-9a-f]{40}", commit) or not re.fullmatch(r"[0-9a-f]{40}", tree):
        raise ValueError("source commit and tree must be full Git object names")
    if run("git", "rev-parse", "HEAD") != commit or run("git", "rev-parse", "HEAD^{tree}") != tree:
        raise ValueError("source identity changed during preparation")
    if run("git", "status", "--porcelain", "--untracked-files=normal"):
        raise ValueError("source checkout must be clean")
    locks = [p for p in run("git", "ls-files").splitlines() if p.endswith("Cargo.lock")]
    return {"commit": commit, "tree": tree, "clean": True,
            "lockfiles": {p: digest(SOURCE_ROOT / p) for p in locks}}


def record(args):
    root = args.root.resolve()
    if (args.root.is_symlink() or not root.is_dir()
            or (root / "platform-input.json").exists()
            or (root / "platform-input.json").is_symlink()):
        raise ValueError("record requires a new, regular staging directory")
    source = source_identity(args.source_commit, args.source_tree)
    if args.target != PLATFORMS[args.platform][1]:
        raise ValueError("native target does not match platform")
    check_version(args.version)
    template = root / "components-template.json"
    reuse = getattr(args, "reuse_support", False)
    if reuse:
        origin = regular_file(root, "support-input.json")
        original = json.loads(origin.read_bytes())
        verify_support_origin(root, original, args.platform, args.version)
        if regular_file(root, "components-template.json").read_bytes() != (SOURCE_ROOT / "components.json").read_bytes():
            raise ValueError("reused template differs from current source")
    elif (root / "support-input.json").exists() or (root / "support-input.json").is_symlink():
        raise ValueError("support provenance requires --reuse-support")
    elif template.exists() or template.is_symlink():
        raise ValueError("template export already exists")
    else:
        template.write_bytes((SOURCE_ROOT / "components.json").read_bytes())
    omissions = json.loads(args.omissions_json.read_text())
    check_contents(root, args.platform, omissions)
    paths = [root / "components.json", template, *sorted((root / "artifacts").rglob("*"))]
    for name in ("upstream-input.json", "upstream-recipes.json", "model-handoff.json"):
        if (root / name).exists():
            paths.append(regular_file(root, name))
    if not reuse and (root / "upstream-recipes.json").exists():
        if (root / "upstream-recipes.json").read_bytes() != (SOURCE_ROOT / "scripts/release-upstream-recipes.json").read_bytes():
            raise ValueError("retained upstream recipes differ from reviewed source")
    if reuse:
        paths.append(origin)
    files = {str(p.relative_to(root)): file_record(regular_file(root, str(p.relative_to(root))))
             for p in paths if not p.is_dir() or p.is_symlink()}
    if source_identity(args.source_commit, args.source_tree) != source:
        raise ValueError("source inputs changed during preparation")
    receipt = {"schema": SCHEMA, "created_at": datetime.now(timezone.utc).isoformat(),
               "source": source, "version": args.version, "platform": args.platform,
               "target": args.target, "omitted_platform_components": omissions,
               "tools": {"rustc": run("rustc", "--version"), "cargo": run("cargo", "--version"),
                         "python": host_platform.python_version()},
               "build_command": ["scripts/prepare-release-platform.sh", "--version", args.version,
                                 "--output", "<output>"],
               "files": files,
               "scope": "unsigned native preparation; external prerequisites and installed journeys require acceptance"}
    if reuse:
        receipt["support_origin"] = {"receipt_path": "support-input.json", "sha256": digest(origin)}
        receipt["build_command"] += ["--reuse-support", "<input>"]
    receipt_path = root / "platform-input.json"
    receipt_path.write_text(json.dumps(receipt, indent=2, sort_keys=True) + "\n")
    try:
        verify(root)
    except Exception:
        receipt_path.unlink()
        raise
    print(f"Recorded {args.platform}: {len(files)} files from {args.source_commit}")


def verify_receipt_header(receipt):
    if not isinstance(receipt, dict):
        raise ValueError("platform receipt must be an object")
    platform = receipt.get("platform")
    if receipt.get("schema") != SCHEMA or not isinstance(platform, str) or platform not in PLATFORMS:
        raise ValueError("unsupported platform input")
    if receipt.get("target") != PLATFORMS[platform][1]:
        raise ValueError("native target does not match platform")
    source = receipt.get("source", {})
    if (not isinstance(source, dict) or source.get("clean") is not True
            or any(not re.fullmatch(r"[0-9a-f]{40}", str(source.get(k, ""))) for k in ("commit", "tree"))):
        raise ValueError("invalid source binding")
    files = receipt.get("files")
    if not isinstance(files, dict) or not files:
        raise ValueError("missing artifact receipt")
    check_version(receipt.get("version"))
    if not isinstance(receipt.get("tools"), dict) or any(
            not isinstance(receipt["tools"].get(tool), str) or not receipt["tools"][tool]
            for tool in ("cargo", "rustc")):
        raise ValueError("missing build tool identity")
    if not isinstance(source.get("lockfiles"), dict) or not source["lockfiles"]:
        raise ValueError("missing lockfile bindings")
    if any(not re.fullmatch(r"[0-9a-f]{64}", str(value)) for value in source["lockfiles"].values()):
        raise ValueError("invalid lockfile binding")
    for relative, expected in files.items():
        if (not isinstance(relative, str) or "\\" in relative
                or any(part in {"", ".", ".."} for part in relative.split("/"))
                or not isinstance(expected, dict) or set(expected) != {"sha256", "size", "executable"}
                or not isinstance(expected["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", expected["sha256"])
                or type(expected["size"]) is not int or expected["size"] <= 0
                or type(expected["executable"]) is not bool):
            raise ValueError(f"invalid artifact receipt: {relative}")
    required = {"components.json", "components-template.json", f"artifacts/elastos-{platform}"}
    if not required <= set(files):
        raise ValueError("native receipt is missing required files")
    runtime = files[f"artifacts/elastos-{platform}"]
    if runtime["size"] < 64 or runtime["executable"] is not True:
        raise ValueError(f"invalid native Runtime record: expected executable for {platform}")
    omissions = receipt.get("omitted_platform_components")
    if (not isinstance(omissions, list) or any(not isinstance(name, str) for name in omissions)
            or len(set(omissions)) != len(omissions)):
        raise ValueError("platform omissions must be unique component names")
    return receipt


def verify_support_origin(root, original, platform, version, current_files=None):
    verify_receipt_header(original)
    if "support_origin" in original or "support-input.json" in original["files"]:
        raise ValueError("nested reused support provenance refused")
    if original["platform"] != platform:
        raise ValueError("support input platform differs")
    if original["version"] == version:
        raise ValueError("support reuse requires a different version")
    runtime = f"artifacts/elastos-{platform}"
    expected = {name: value for name, value in original["files"].items() if name != runtime}
    actual = {str(path.relative_to(root)): file_record(regular_file(root, str(path.relative_to(root))))
              for path in root.rglob("*") if (not path.is_dir() or path.is_symlink())
              and str(path.relative_to(root)) not in {runtime, "platform-input.json", "support-input.json"}}
    if actual != expected:
        raise ValueError("reused support files differ from original receipt")
    if current_files is not None and {name: value for name, value in current_files.items()
                                    if name not in {runtime, "support-input.json"}} != expected:
        raise ValueError("reused support records differ from original receipt")
    return original


def verify(root):
    if root.is_symlink() or not root.is_dir():
        raise ValueError("input root must be a regular directory")
    receipt = verify_receipt_header(json.loads(regular_file(root, "platform-input.json").read_text()))
    platform, files = receipt["platform"], receipt["files"]
    actual = {str(path.relative_to(root)) for path in root.rglob("*")
              if (not path.is_dir() or path.is_symlink()) and path != root / "platform-input.json"}
    if actual != set(files):
        raise ValueError("input file inventory differs from receipt")
    for relative, expected in files.items():
        if (not isinstance(expected, dict) or type(expected.get("size")) is not int
                or expected["size"] <= 0 or type(expected.get("executable")) is not bool):
            raise ValueError(f"invalid artifact receipt: {relative}")
        if file_record(regular_file(root, relative)) != expected:
            raise ValueError(f"input artifact differs from receipt: {relative}")
    check_contents(root, platform, receipt.get("omitted_platform_components"))
    if "support_origin" in receipt:
        origin = receipt["support_origin"]
        if (not isinstance(origin, dict) or set(origin) != {"receipt_path", "sha256"}
                or origin["receipt_path"] != "support-input.json"
                or not isinstance(origin["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", origin["sha256"])):
            raise ValueError("invalid support provenance")
        original_path = regular_file(root, origin["receipt_path"])
        if digest(original_path) != origin["sha256"]:
            raise ValueError("support provenance receipt hash differs")
        original = verify_support_origin(root, json.loads(original_path.read_bytes()), platform, receipt["version"], files)
        if receipt.get("omitted_platform_components") != original.get("omitted_platform_components"):
            raise ValueError("reused platform omissions differ from original receipt")
    elif "support-input.json" in files:
        raise ValueError("support input lacks provenance binding")
    if "model_finalization" in receipt:
        verify_model_finalization(root, receipt)
    elif any(name in files for name in ("model-native-input.json", "model-native-components.json", "model-native-catalog.json")):
        raise ValueError("model native provenance lacks finalization binding")
    return receipt


def directory_descriptor(path):
    """Open every directory through a held parent; symlinks are refused."""
    path = Path(os.path.abspath(path))
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY)
    try:
        for part in path.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        return fd
    except BaseException:
        os.close(fd)
        raise


def support_file_descriptor(root_fd, relative, flags, created_dirs=None, root=None):
    # Validate the relative name before using it with directory descriptors.
    if "\\" in relative or any(part in {"", ".", ".."} for part in relative.split("/")):
        raise ValueError("unsafe support path")
    fd = os.dup(root_fd)
    try:
        prefix = []
        for part in relative.split("/")[:-1]:
            prefix.append(part)
            if created_dirs is not None:
                try:
                    os.mkdir(part, mode=0o700, dir_fd=fd)
                    created_dirs.append(root.joinpath(*prefix))
                except FileExistsError:
                    pass
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
        return os.open(relative.split("/")[-1], flags | os.O_NOFOLLOW, 0o600, dir_fd=fd)
    finally:
        os.close(fd)


def copy_support(args):
    original_root, root = args.input, args.root
    source_fd = directory_descriptor(original_root)
    destination_fd = None
    copied, directories, source_stamps = [], [], {}
    try:
        destination_fd = directory_descriptor(root)
        receipt_path = regular_file(original_root, "platform-input.json")
        receipt_bytes = receipt_path.read_bytes()
        original = verify(original_root)
        if original != json.loads(receipt_bytes):
            raise ValueError("support receipt changed during admission")
        if "support_origin" in original or "support-input.json" in original["files"]:
            raise ValueError("nested reused support provenance refused")
        if original["platform"] != args.platform:
            raise ValueError("support input platform differs")
        check_version(args.version)
        if original["version"] == args.version:
            raise ValueError("support reuse requires a different version")
        if regular_file(original_root, "components-template.json").read_bytes() != (SOURCE_ROOT / "components.json").read_bytes():
            raise ValueError("support template differs from current source")
        runtime = f"artifacts/elastos-{args.platform}"
        records = {name: value for name, value in original["files"].items() if name != runtime}
        receipt_record = file_record(receipt_path)
        if receipt_record["sha256"] != hashlib.sha256(receipt_bytes).hexdigest():
            raise ValueError("support receipt changed during admission")
        usage = shutil.disk_usage(root)
        required = sum(item["size"] for item in records.values()) + len(receipt_bytes)
        if (usage.free - required) * 100 < usage.total * 15:
            raise ValueError("support reuse requires 15% free after its copy")
        for name, expected in [*records.items(), ("platform-input.json", receipt_record)]:
            target = "support-input.json" if name == "platform-input.json" else name
            input_fd = support_file_descriptor(source_fd, name, os.O_RDONLY | os.O_NONBLOCK)
            try:
                before = os.fstat(input_fd)
                if not stat.S_ISREG(before.st_mode):
                    raise ValueError("support input is not a regular file")
                with os.fdopen(os.dup(input_fd), "rb") as stream:
                    initial_hash = hashlib.sha256()
                    for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                        initial_hash.update(chunk)
                    if {"sha256": initial_hash.hexdigest(), "size": before.st_size,
                            "executable": bool(before.st_mode & 0o111)} != expected:
                        raise ValueError("support input changed before copy")
                    stream.seek(0)
                    output_fd = support_file_descriptor(destination_fd, target, os.O_WRONLY | os.O_CREAT | os.O_EXCL, directories, root)
                    copied.append(root / target)
                    with os.fdopen(output_fd, "wb") as output:
                        shutil.copyfileobj(stream, output, 1024 * 1024)
                        os.fchmod(output.fileno(), stat.S_IMODE(before.st_mode))
                after = os.fstat(input_fd)
                if (before.st_dev, before.st_ino, before.st_size, before.st_mode, before.st_mtime_ns, before.st_ctime_ns) != (
                        after.st_dev, after.st_ino, after.st_size, after.st_mode, after.st_mtime_ns, after.st_ctime_ns):
                    raise ValueError("support input changed during copy")
                source_stamps[name] = (after.st_dev, after.st_ino, after.st_size, after.st_mode,
                                       after.st_mtime_ns, after.st_ctime_ns)
                if file_record(regular_file(original_root, name)) != expected or file_record(regular_file(root, target)) != expected:
                    raise ValueError("support bytes changed during copy")
                current_fd = support_file_descriptor(source_fd, name, os.O_RDONLY | os.O_NONBLOCK)
                try:
                    current = os.fstat(current_fd)
                    if (current.st_dev, current.st_ino, current.st_mode) != (after.st_dev, after.st_ino, after.st_mode):
                        raise ValueError("support input identity changed during copy")
                finally:
                    os.close(current_fd)
            finally:
                os.close(input_fd)
        if receipt_path.read_bytes() != receipt_bytes or verify(original_root) != original:
            raise ValueError("support receipt or input changed during copy")
        for name, stamp in source_stamps.items():
            current_fd = support_file_descriptor(source_fd, name, os.O_RDONLY | os.O_NONBLOCK)
            try:
                current = os.fstat(current_fd)
                if (current.st_dev, current.st_ino, current.st_size, current.st_mode,
                        current.st_mtime_ns, current.st_ctime_ns) != stamp:
                    raise ValueError("support input changed after copy")
            finally:
                os.close(current_fd)
        verify_support_origin(root, original, args.platform, args.version)
        if (root / "support-input.json").read_bytes() != receipt_bytes:
            raise ValueError("copied support receipt differs")
        return original
    except BaseException:
        for path in reversed(copied):
            path.unlink()
        for path in reversed(directories):
            path.rmdir()
        raise
    finally:
        os.close(source_fd)
        if destination_fd is not None:
            os.close(destination_fd)


def selected_platforms(preview_platform=None, provided=None):
    """The platform set one publication admits.

    A preview admits exactly one named platform. A stable publication admits
    every supplied release platform when at least two are present. With no
    provided set, the default remains all three release platforms.
    """
    if preview_platform is not None:
        if preview_platform not in PLATFORMS:
            raise ValueError(f"preview platform is not a release platform: {preview_platform}")
        return {preview_platform}
    if provided is None:
        return set(PLATFORMS)
    selected = set(provided)
    if not selected <= set(PLATFORMS):
        raise ValueError("inputs require each full release platform exactly once")
    if len(selected) < 2:
        raise ValueError("candidate input requires all three platforms")
    return selected


def validate_inputs(values, version=None, preview_platform=None):
    preview_selected = (
        selected_platforms(preview_platform) if preview_platform is not None else None)
    inputs = {}
    for value in values:
        name, sep, path = value.partition("=")
        if not sep or name not in PLATFORMS or name in inputs:
            raise ValueError("inputs require each full release platform exactly once")
        if preview_selected is not None and name not in preview_selected:
            raise ValueError(f"input platform is outside the selected publication: {name}")
        receipt = verify(Path(path))
        if receipt["platform"] != name:
            raise ValueError("input label differs from receipt platform")
        manifest = json.loads((Path(path) / "components.json").read_text())
        if "upstream-input.json" in receipt["files"]:
            upstream = json.loads(regular_file(Path(path), "upstream-input.json").read_bytes())
            prepared_models = {item["component"] for item in upstream.get("capsules", [])} & MODEL_COMPONENTS
            if prepared_models and (prepared_models != MODEL_COMPONENTS
                    or set(manifest.get("model_retention", {})) != MODEL_COMPONENTS):
                raise ValueError("model publication requires the finalized four-model catalogue and retention set")
        for component_name in manifest["profiles"]["home"]["components"]:
            component = manifest["external"].get(component_name)
            if component is None:
                raise ValueError(f"{name}: required Home component is absent: {component_name}")
            _, info = integrity.resolve_platform_info(component, PLATFORMS[name][0])
            if info is None:
                if component_name not in receipt["omitted_platform_components"]:
                    raise ValueError(f"{name}: unrecorded platform omission: {component_name}")
            elif info.get("strategy") in integrity.DEV_STRATEGIES:
                raise ValueError(f"{name}: required Home component needs a distributable artifact: {component_name}")
            elif not any(info.get(key) for key in ("release_path", "url")):
                raise ValueError(f"{name}: required Home component has no prepared delivery path: {component_name}")
        if "upstream-recipes.json" in receipt["files"]:
            origin = receipt
            if "support_origin" in receipt:
                origin = json.loads(regular_file(Path(path), "support-input.json").read_bytes())
            reviewed = subprocess.check_output(["git", "show", origin["source"]["commit"] + ":scripts/release-upstream-recipes.json"], cwd=SOURCE_ROOT)
            if hashlib.sha256(reviewed).hexdigest() != receipt["files"]["upstream-recipes.json"]["sha256"]:
                raise ValueError("upstream recipe pins differ from their original reviewed source")
        inputs[name] = receipt
    selected = selected_platforms(preview_platform, inputs)
    if set(inputs) != selected:
        if preview_platform is None:
            raise ValueError("candidate input requires all three platforms")
        raise ValueError(f"preview input requires exactly one {preview_platform} input")
    first = next(iter(inputs.values()))
    if version is not None and first["version"] != version:
        raise ValueError("platform input version differs from requested release")
    # Admission runs from the reviewed candidate checkout. Agreement between
    # unsigned workers is insufficient to establish the selected source tree.
    expected_source = source_identity(first["source"]["commit"], first["source"]["tree"])
    if first["source"] != expected_source:
        raise ValueError("input source/lockfiles differ from the candidate checkout")
    expected_template = digest(SOURCE_ROOT / "components.json")
    if first["files"]["components-template.json"]["sha256"] != expected_template:
        raise ValueError("input template differs from the candidate checkout")
    for receipt in inputs.values():
        if (receipt["source"] != first["source"] or receipt["version"] != first["version"]
                or receipt["files"]["components-template.json"] != first["files"]["components-template.json"]
                or any(receipt["tools"].get(t) != first["tools"].get(t) for t in ("rustc", "cargo"))):
            raise ValueError("platform source/version/template/toolchain mismatch")
    # Same-named local files are universal assets; native names include platform.
    shared = {}
    for receipt in inputs.values():
        for path, info in receipt["files"].items():
            if path.startswith("artifacts/"):
                if path in shared and shared[path] != info:
                    raise ValueError(f"conflicting universal asset: {path}")
                shared[path] = info
    return inputs


def merged_input_components(values, receipts):
    merged = json.loads((SOURCE_ROOT / "components.json").read_text())
    merged["schema"], merged["capsules"] = "elastos.components/v1", {}
    selected = {}
    model_contract = None
    for value in values:
        platform, _, path = value.partition("=")
        manifest_path = regular_file(Path(path), "components.json")
        manifest_bytes = manifest_path.read_bytes()
        expected = receipts[platform]["files"]["components.json"]
        actual = {"sha256": hashlib.sha256(manifest_bytes).hexdigest(), "size": len(manifest_bytes),
                  "executable": bool(manifest_path.stat().st_mode & 0o111)}
        if actual != expected:
            raise ValueError(f"input manifest changed after admission: {platform}")
        manifest = json.loads(manifest_bytes)
        contract = {name: manifest.get(name) for name in ("model_catalog", "model_retention")}
        if model_contract is not None and contract != model_contract:
            raise ValueError("platform model catalogue/retention contracts differ")
        model_contract = contract
        for name, value in contract.items():
            if value is None:
                merged.pop(name, None)
            else:
                merged[name] = copy.deepcopy(value)
        for name, component in manifest["external"].items():
            for metadata in (False, True):
                entry = component.get("capsule_metadata") if metadata else component
                if entry is None:
                    continue
                key, info = integrity.resolve_platform_info(entry, PLATFORMS[platform][0])
                if info is None:
                    continue
                identity = (name, metadata, key)
                if identity in selected and selected[identity] != info:
                    raise ValueError(f"conflicting universal descriptor: {name} ({key})")
                selected[identity] = info
                target = merged["external"][name]
                if metadata:
                    contract = {k: v for k, v in entry.items() if k != "platforms"}
                    target = target.setdefault("capsule_metadata", {**contract, "platforms": {}})
                    if {k: v for k, v in target.items() if k != "platforms"} != contract:
                        raise ValueError(f"conflicting provider metadata contract: {name}")
                target.setdefault("platforms", {})[key] = copy.deepcopy(info)
    # A publication advertises only the platforms it admitted. Template
    # descriptors for the other release platforms would name artifacts this
    # publication never stages, so they leave the merged manifest here.
    unselected = {PLATFORMS[platform][0] for platform in PLATFORMS if platform not in receipts}
    for component in merged["external"].values():
        for entry in (component, component.get("capsule_metadata")):
            if isinstance(entry, dict):
                for setup in unselected:
                    entry.get("platforms", {}).pop(setup, None)
    return merged


def stage_inputs(values, version, output, preview_platform=None):
    receipts = validate_inputs(values, version, preview_platform)
    if output.exists() or output.is_symlink():
        raise ValueError("publication input staging output already exists")
    merged = merged_input_components(values, receipts)
    files = {}
    origins = {}
    for value in values:
        platform, _, path = value.partition("=")
        for relative, record in receipts[platform]["files"].items():
            if relative.startswith("artifacts/"):
                name = relative.removeprefix("artifacts/")
                files[name], origins[name] = record, (Path(path), relative)
    parent = output.parent.resolve()
    parent.mkdir(parents=True, exist_ok=True)
    usage = shutil.disk_usage(parent)
    if (usage.free - sum(record["size"] for record in files.values())) * 100 < usage.total * 15:
        raise ValueError("publication staging requires at least 15% free after its copy")
    with tempfile.TemporaryDirectory(prefix=".platform-import-", dir=parent) as temporary:
        stage = Path(temporary) / "input"
        artifacts = stage / "artifacts"
        artifacts.mkdir(parents=True)
        for name, (root, relative) in origins.items():
            destination = artifacts / name
            shutil.copyfile(regular_file(root, relative), destination)
            destination.chmod(0o700 if files[name]["executable"] else 0o600)
            if file_record(destination) != files[name]:
                raise ValueError(f"input changed while staging: {name}")
        (stage / "components.json").write_text(json.dumps(merged, indent=2) + "\n")
        first = next(iter(receipts.values()))
        record = {"version": version, "source": first["source"],
                  "platforms": sorted(receipts), "files": files,
                  "components": file_record(stage / "components.json")}
        (stage / "assembly.json").write_text(json.dumps(record, indent=2) + "\n")
        verify_staged_inputs(stage, preview_platform=preview_platform)
        stage.rename(output)
    return record


def verify_staged_inputs(stage, allow_generated=False, preview_platform=None):
    record = json.loads(regular_file(stage, "assembly.json").read_text())
    source = record["source"]
    if source_identity(source["commit"], source["tree"]) != source:
        raise ValueError("publication staging source differs from candidate checkout")
    admitted = None if preview_platform else record["platforms"]
    if set(record["platforms"]) != selected_platforms(preview_platform, admitted):
        if preview_platform is None:
            raise ValueError("publication staging requires all three platforms")
        raise ValueError(f"publication staging is not the {preview_platform} preview")
    actual = {str(path.relative_to(stage / "artifacts"))
              for path in (stage / "artifacts").rglob("*") if not path.is_dir() or path.is_symlink()}
    generated = {f"components-{platform}.json" for platform in record["platforms"]} if allow_generated else set()
    if not set(record["files"]) <= actual or actual - set(record["files"]) - generated:
        raise ValueError("staged artifact inventory differs from admitted inputs")
    for name, expected in record["files"].items():
        if file_record(regular_file(stage / "artifacts", name)) != expected:
            raise ValueError(f"staged input differs from admitted bytes: {name}")
    components = regular_file(stage, "components.json")
    if file_record(components) != record["components"]:
        raise ValueError("staged components changed after admission")
    manifest = json.loads(components.read_text())
    for platform in record["platforms"]:
        setup = PLATFORMS[platform][0]
        check_binary(regular_file(stage / "artifacts", f"elastos-{platform}"), platform)
        errors = integrity.audit_manifest(manifest, [setup])
        errors += integrity.audit_release_artifacts(manifest, [setup], stage / "artifacts")
        if errors:
            raise ValueError("; ".join(errors))
    return record


def prepared_components_bytes(stage, record, cids):
    if set(cids) != set(record["files"]) or any(
            not isinstance(cid, str) or not re.fullmatch(r"[A-Za-z0-9]+", cid) for cid in cids.values()):
        raise ValueError("upload results must bind every admitted artifact to a nonempty CID")
    manifest = json.loads((stage / "components.json").read_text())
    for component in manifest["external"].values():
        for entry in (component, component.get("capsule_metadata", {})):
            for info in entry.get("platforms", {}).values():
                if info.get("release_path"):
                    info["cid"] = cids[info["release_path"]]
    for model in manifest.get("model_retention", {}).values():
        for kind in ("car", "receipt"):
            descriptor = model[kind]
            descriptor["cid"] = cids[descriptor["release_path"]]
    # Each selected platform gets the same complete manifest. Descriptors were
    # replaced as units during staging; each release descriptor names admitted bytes.
    output_bytes = (json.dumps(manifest, indent=2) + "\n").encode()
    for platform in record["platforms"]:
        setup = PLATFORMS[platform][0]
        existing = stage / "artifacts" / f"components-{platform}.json"
        if existing.exists() or existing.is_symlink():
            if regular_file(stage / "artifacts", existing.name).read_bytes() != output_bytes:
                raise ValueError(f"existing generated components differ: {platform}")
        errors = integrity.audit_manifest(manifest, [setup])
        errors += integrity.audit_release_artifacts(manifest, [setup], stage / "artifacts")
        if errors:
            raise ValueError("; ".join(errors))
    return output_bytes


def attach_input_cids(stage, cids_path, preview_platform=None):
    record = verify_staged_inputs(stage, allow_generated=True, preview_platform=preview_platform)
    cids = json.loads(cids_path.read_text())
    output_bytes = prepared_components_bytes(stage, record, cids)
    for platform in record["platforms"]:
        output = stage / "artifacts" / f"components-{platform}.json"
        if not output.exists():
            output.write_bytes(output_bytes)
    return record


def installer_source_blob(source):
    """Read the selected installer as inert Git data, not working-tree code."""
    oid = run("git", "rev-parse", source["commit"] + ":scripts/install.sh")
    data = subprocess.check_output(["git", "cat-file", "blob", oid], cwd=SOURCE_ROOT)
    actual = hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest()
    if not re.fullmatch(r"[0-9a-f]{40}", oid) or actual != oid:
        raise ValueError("installer source blob differs")
    return oid, data


def signing_input(stage, cids_path, stamps_path, channel, output,
                  preview_platform=None, prev_release_cid=None, prev_head_cid=None):
    """Prepare data for the separately installed custodian signer; no key input."""
    spec = importlib.util.spec_from_file_location("release_signer", SCRIPT_ROOT / "release-signer.py")
    signer = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = signer
    spec.loader.exec_module(signer)
    record = verify_staged_inputs(stage, allow_generated=True, preview_platform=preview_platform)
    signer.require(channel in signer.CHANNELS, "release channel refused")
    signer.require(preview_platform is None or channel == "canary", "preview requires canary")
    source = {name: record["source"][name] for name in ("commit", "tree")}
    cids = signer.parse_json(cids_path.read_bytes())
    stamps = signer.parse_json(stamps_path.read_bytes())
    signer.check_did(stamps.get("MAINTAINER_DID"))
    blob_oid, template = installer_source_blob(source)
    rendered = signer.render_installer(template, stamps, stamps["MAINTAINER_DID"])
    names = set(record["files"]) | {f"components-{p}.json" for p in record["platforms"]}
    signer.require(set(cids) == names, "CID results must bind the complete artifact set")
    expected_components = prepared_components_bytes(stage, record,
        {name: cids[name] for name in record["files"]})
    for platform in record["platforms"]:
        signer.require(regular_file(stage / "artifacts", f"components-{platform}.json").read_bytes()
                       == expected_components, "generated components differ from admitted inputs")
    files = {}
    for name in sorted(names):
        signer.relative_path(name)
        info = file_record(regular_file(stage / "artifacts", name))
        codec, cid_digest = signer.cid_info(cids[name])
        signer.require(codec != 0x55 or cid_digest.hex() == info["sha256"], "raw CID differs from artifact")
        files[name] = {"sha256": info["sha256"], "size": info["size"], "cid": cids[name]}
    for previous in (prev_release_cid, prev_head_cid):
        if previous is not None:
            signer.cid_info(previous)
    platforms = {p: {kind: files[name] for kind, name in (
        ("binary", f"elastos-{p}"), ("components", f"components-{p}.json"))}
        for p in record["platforms"]}
    now = int(datetime.now(timezone.utc).timestamp())
    manifest = {"source": source, "version": record["version"], "channel": channel,
                "files": files, "installer": {"blob_oid": blob_oid, "stamps": stamps},
                "release": {"schema": "elastos.release/v1", "source": source,
                            "version": record["version"], "channel": channel,
                            "released_at": now, "prev_release_cid": prev_release_cid,
                            "platforms": platforms, "installer_sha256": signer.sha256(rendered)},
                "head": {"updated_at": now, "prev_head_cid": prev_head_cid}}
    manifest_bytes = signer.json_bytes(manifest)
    signer.require(len(manifest_bytes) <= signer.MAX_JSON, "signing input too large")
    if output.exists() or output.is_symlink():
        raise ValueError("signing input output already exists")
    parent = output.parent.resolve(strict=True)
    destination = parent / output.name
    signer.require(not destination.is_relative_to(stage.resolve()), "signing input output must be outside staging")
    usage = shutil.disk_usage(parent)
    total = sum(info["size"] for info in files.values()) + len(manifest_bytes)
    signer.require((usage.free - total) * 100 >= usage.total * 15, "signing input requires 15 percent free after its copy")
    with tempfile.TemporaryDirectory(prefix=".signing-input-", dir=parent) as temporary:
        prepared = Path(temporary) / "input"
        prepared.mkdir(mode=0o700)
        for name, info in files.items():
            path = prepared / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(regular_file(stage / "artifacts", name), path)
            signer.require(digest(path) == info["sha256"] and path.stat().st_size == info["size"],
                           "artifact changed while preparing signing input")
            path.chmod(0o400)
        (prepared / "signing-input.json").write_bytes(manifest_bytes)
        (prepared / "signing-input.json").chmod(0o400)
        verify_staged_inputs(stage, allow_generated=True, preview_platform=preview_platform)
        # rename refuses an existing non-empty destination; refuse all existing
        # destinations here, including empty directories and symlinks.
        signer.require(not destination.exists() and not destination.is_symlink(), "signing input output already exists")
        prepared.rename(destination)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    create = commands.add_parser("record")
    create.add_argument("--reuse-support", action="store_true")
    for name in ("root", "omissions-json"):
        create.add_argument("--" + name, type=Path, required=True)
    for name in ("version", "target", "source-commit", "source-tree"):
        create.add_argument("--" + name, required=True)
    create.add_argument("--platform", choices=PLATFORMS, required=True)
    check = commands.add_parser("verify")
    check.add_argument("root", type=Path)
    reuse = commands.add_parser("copy-support", help="copy verified native support without the original Runtime")
    for name in ("input", "root"):
        reuse.add_argument("--" + name, type=Path, required=True)
    reuse.add_argument("--platform", choices=PLATFORMS, required=True)
    reuse.add_argument("--version", required=True)
    finalize = commands.add_parser("finalize-models", help="verify public catalogue and retained CAR bytes into a new native input")
    for name in ("input", "handoff", "catalogue", "output", "openssl"):
        finalize.add_argument("--" + name, type=Path, required=True)
    finalize.add_argument("--publisher-did", required=True)
    combined = commands.add_parser("validate-inputs")
    combined.add_argument("--input", action="append", required=True)
    combined.add_argument("--version")
    stage = commands.add_parser("stage-inputs")
    stage.add_argument("--input", action="append", required=True)
    stage.add_argument("--version", required=True)
    stage.add_argument("--output", required=True, type=Path)
    staged = commands.add_parser("verify-staged")
    staged.add_argument("root", type=Path)
    attach = commands.add_parser("attach-cids")
    attach.add_argument("root", type=Path)
    attach.add_argument("--cids", required=True, type=Path)
    prepare = commands.add_parser("signing-input", help="prepare unsigned data for the custodian signer")
    prepare.add_argument("root", type=Path)
    prepare.add_argument("--cids", required=True, type=Path)
    prepare.add_argument("--stamps", required=True, type=Path)
    prepare.add_argument("--channel", required=True)
    prepare.add_argument("--output", required=True, type=Path)
    prepare.add_argument("--prev-release-cid")
    prepare.add_argument("--prev-head-cid")
    for command in (combined, stage, staged, attach, prepare):
        command.add_argument("--preview-platform", choices=PLATFORMS,
                             help="admit exactly this one native input instead of all release platforms")
    args = parser.parse_args()
    try:
        if args.command == "record":
            record(args)
        elif args.command == "copy-support":
            copy_support(args)
        elif args.command == "finalize-models":
            finalize_models(args)
            print("Verified public catalogue and CAR receipt bytes in a new native input; consumer acceptance remains separate.")
        elif args.command == "verify":
            print(json.dumps(verify(args.root), sort_keys=True))
        elif args.command == "stage-inputs":
            stage_inputs(args.input, args.version, args.output, args.preview_platform)
        elif args.command == "verify-staged":
            verify_staged_inputs(args.root, preview_platform=args.preview_platform)
        elif args.command == "attach-cids":
            attach_input_cids(args.root, args.cids, args.preview_platform)
        elif args.command == "signing-input":
            signing_input(args.root, args.cids, args.stamps, args.channel, args.output,
                          args.preview_platform, args.prev_release_cid, args.prev_head_cid)
            print("Prepared unsigned signing input: " + hashlib.sha256(
                (args.output / "signing-input.json").read_bytes()).hexdigest())
        else:
            receipts = validate_inputs(args.input, args.version, args.preview_platform)
            print(f"Verified source and local bytes for {len(receipts)} platform inputs; publication and installed acceptance remain separate.")
    except (ValueError, OSError, KeyError, TypeError, tarfile.TarError, subprocess.CalledProcessError) as exc:
        parser.exit(1, f"Error: {exc}\n")


if __name__ == "__main__":
    main()
