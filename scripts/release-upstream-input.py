#!/usr/bin/env python3
"""Wrap pinned build inputs as passive release capsules. Never installs or signs.

The build-only recipe owns upstream URLs and licence pins. The returned receipt
owns the package checksum; release publication supplies its Carrier CID.
"""

import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
import zipfile


SCHEMA = "elastos.release-upstream-input/v1"
CHUNK = 1024 * 1024
MAX_BYTES = 16 * 1024**3
METADATA_BYTES = 1024**2
MAX_FILES = 4096


def canonical(value):
    return (json.dumps(value, sort_keys=True, separators=(",", ":"),
                       ensure_ascii=False) + "\n").encode()


def relative(value):
    if (not isinstance(value, str) or len(value) > 240 or "\\" in value
            or PurePosixPath(value).is_absolute()
            or any(not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._+-]*", part)
                   or part in (".", "..") for part in value.split("/"))):
        raise ValueError(f"unsafe package path: {value!r}")
    return value


def regular(path):
    path = Path(path)
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise ValueError(f"symlink in build input path: {path}")
    metadata = path.lstat()
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise ValueError(f"build input must be a single-link regular file: {path}")
    return path


def directory(path):
    path = Path(path).absolute()
    for parent in [path, *path.parents]:
        if parent.is_symlink():
            raise ValueError(f"symlink in build directory: {path}")
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = path.stat()
    if (not path.is_dir() or metadata.st_uid != os.geteuid()
            or metadata.st_mode & 0o022):
        raise ValueError(f"build directory must be owned and protected: {path}")
    return path


def disk_gate(path, additional):
    usage = shutil.disk_usage(path)
    if (usage.free - additional) * 100 < usage.total * 15:
        raise ValueError("build input would leave less than 15% free disk space")


def checksum_parts(checksum):
    if not isinstance(checksum, str) or not re.fullmatch(
            r"sha256:[0-9a-f]{64}|sha512:[0-9a-f]{128}", checksum):
        raise ValueError("upstream source requires a recorded SHA-256 or SHA-512 pin")
    return checksum.split(":", 1)


def digest(path, algorithm="sha256"):
    value = hashlib.new(algorithm)
    with regular(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(CHUNK), b""):
            value.update(chunk)
    return value.hexdigest()


def source_spec(source, limit=MAX_BYTES):
    if not isinstance(source, dict) or set(source) - {"url", "path", "checksum", "max_bytes", "redirect_hosts"}:
        raise ValueError("unknown upstream source fields")
    if ("url" in source) == ("path" in source):
        raise ValueError("upstream source requires exactly one URL or local path")
    checksum_parts(source.get("checksum"))
    maximum = source.get("max_bytes")
    if type(maximum) is not int or not 0 < maximum <= limit:
        raise ValueError("upstream source requires a positive bounded max_bytes")
    hosts = source.get("redirect_hosts", [])
    if not isinstance(hosts, list) or len(hosts) > 16:
        raise ValueError("invalid upstream redirect host allowlist")
    for host in hosts:
        if not isinstance(host, str) or not re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", host):
            raise ValueError("invalid upstream redirect host")
    if "url" in source:
        validate_url(source["url"])
    elif not isinstance(source["path"], str) or not Path(source["path"]).is_absolute():
        raise ValueError("local upstream input path must be absolute")
    return source


def validate_url(url):
    if not isinstance(url, str) or len(url) > 8192 or any(ord(c) < 32 for c in url):
        raise ValueError("unsafe upstream URL")
    parsed = urllib.parse.urlsplit(url)
    if (parsed.scheme != "https" or not parsed.hostname or parsed.username
            or parsed.password or parsed.fragment or parsed.port not in (None, 443)
            or not re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", parsed.hostname)
            or parsed.hostname == "localhost" or parsed.hostname.endswith(".localhost")
            or re.fullmatch(r"[0-9.]+", parsed.hostname)):
        raise ValueError("upstream URL must use public HTTPS without credentials")
    return parsed.hostname


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def response(source):
    """Only HTTPS redirects to recipe-owned hosts; each response has a bound."""
    url = source["url"]
    allowed = {validate_url(url), *source.get("redirect_hosts", [])}
    opener = urllib.request.build_opener(NoRedirect())
    for hop in range(6):
        if validate_url(url) not in allowed:
            raise ValueError("upstream redirect host is outside the recorded allowlist")
        request = urllib.request.Request(url, headers={"User-Agent": "ElastOS-release-input", "Accept-Encoding": "identity"})
        try:
            reply = opener.open(request, timeout=30)
        except urllib.error.HTTPError as error:
            if error.code not in (301, 302, 303, 307, 308) or hop == 5:
                error.close()
                raise
            location = error.headers.get("Location")
            error.close()
            if not location:
                raise ValueError("upstream redirect has no target")
            url = urllib.parse.urljoin(url, location)
            continue
        if reply.status != 200 or reply.headers.get("Content-Encoding", "identity") != "identity":
            reply.close()
            raise ValueError("upstream response must be unencoded HTTP 200")
        length = reply.headers.get("Content-Length")
        if length is not None and (not length.isdecimal() or not 0 < int(length) <= source["max_bytes"]):
            reply.close()
            raise ValueError("upstream response exceeds its recorded size bound")
        return reply
    raise ValueError("too many upstream redirects")


def cached_input(source, cache, limit=MAX_BYTES):
    source_spec(source, limit)
    cache = directory(cache)
    algorithm, expected = checksum_parts(source["checksum"])
    destination = cache / f"{algorithm}-{expected}"
    if destination.exists() or destination.is_symlink():
        if not 0 < regular(destination).stat().st_size <= source["max_bytes"] or digest(destination, algorithm) != expected:
            raise ValueError("cached upstream input failed its recorded checksum or bound")
        return destination
    disk_gate(cache, source["max_bytes"])
    fd, temporary = tempfile.mkstemp(prefix=".upstream-", dir=cache)
    temporary = Path(temporary)
    try:
        value, size = hashlib.new(algorithm), 0
        with os.fdopen(fd, "wb") as output:
            stream = regular(source["path"]).open("rb") if "path" in source else response(source)
            with stream:
                while chunk := stream.read(CHUNK):
                    size += len(chunk)
                    if size > source["max_bytes"]:
                        raise ValueError("upstream input exceeds its recorded size bound")
                    value.update(chunk)
                    output.write(chunk)
            output.flush()
            os.fsync(output.fileno())
        if size == 0 or value.hexdigest() != expected:
            raise ValueError("upstream input checksum mismatch")
        if destination.exists() or destination.is_symlink():
            raise ValueError("upstream cache destination changed during download")
        os.link(temporary, destination)
        temporary.unlink()
        return destination
    finally:
        temporary.unlink(missing_ok=True)


def link_target(name, target, root, symbolic):
    """Tar symlinks are parent-relative; hardlinks name archive members."""
    if (not isinstance(target, str) or not target or len(target) > 240
            or "\\" in target or target.startswith("/")):
        raise ValueError("unsafe upstream archive link target")
    parts = name.split("/")[:-1] if symbolic else []
    for part in target.split("/"):
        if part == ".":
            continue
        if part == "..":
            if len(parts) <= 1:
                raise ValueError("upstream archive link escapes its capsule root")
            parts.pop()
        else:
            relative(part)
            parts.append(part)
    resolved = relative("/".join(parts))
    if resolved != root and not resolved.startswith(root + "/"):
        raise ValueError("upstream archive link escapes its capsule root")
    return resolved


def archive_members(path, kind, root, maximum):
    """Validate the whole archive before creating any package output."""
    files, seen, total = {}, set(), 0
    archive = tarfile.open(path, "r|gz") if kind == "tar.gz" else zipfile.ZipFile(path)
    with archive:
        entries = archive if kind == "tar.gz" else archive.infolist()
        for entry in entries:
            name = relative((entry.name if kind == "tar.gz" else entry.filename).rstrip("/"))
            if name != root and not name.startswith(root + "/"):
                raise ValueError("upstream archive escapes its recorded capsule root")
            if name in seen or len(seen) >= MAX_FILES:
                raise ValueError("duplicate or excessive upstream archive members")
            if any(str(parent) in files for parent in PurePosixPath(name).parents):
                raise ValueError("upstream archive member is beneath a file")
            if kind == "tar.gz":
                is_file = entry.type in (tarfile.REGTYPE, tarfile.AREGTYPE)
                is_dir, size, mode = entry.isdir(), entry.size, entry.mode
                is_link = entry.issym() or entry.islnk()
                if not (is_file or is_dir or is_link) or entry.pax_headers:
                    raise ValueError("upstream archive special files or extended headers refused")
            else:
                mode = entry.external_attr >> 16
                is_dir, size = entry.is_dir(), entry.file_size
                is_file, is_link = not is_dir, False
                if stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR) or entry.flag_bits & 1:
                    raise ValueError("upstream ZIP links, special or encrypted files refused")
            if is_file or is_link:
                if any(other.startswith(name + "/") for other in seen):
                    raise ValueError("upstream archive file is empty or replaces a parent")
                if is_link:
                    if size != 0:
                        raise ValueError("upstream archive link contains payload bytes")
                    files[name] = {"target": link_target(name, entry.linkname, root, entry.issym())}
                else:
                    if size <= 0:
                        raise ValueError("upstream archive file is empty")
                    total += size
                    if total > maximum:
                        raise ValueError("upstream archive exceeds its unpacked size bound")
                    files[name] = {"size": size, "mode": 0o755 if mode & 0o111 else 0o644,
                                   "source": name}
            elif size != 0:
                raise ValueError("upstream archive directory contains payload bytes")
            seen.add(name)
    if not files:
        raise ValueError("upstream archive contains no regular payload files")
    # Resolve only exact file aliases. Directory aliases and aliased parents are
    # refused, so extraction never follows a link. Copies inherit the file mode.
    resolved, total = {}, 0
    for name in files:
        current, chain = name, []
        while current not in resolved:
            if current in chain:
                raise ValueError("upstream archive link cycle")
            if any(str(parent) in files for parent in PurePosixPath(current).parents):
                raise ValueError("upstream archive link target is beneath a file or alias")
            record = files.get(current)
            if record is None:
                raise ValueError("upstream archive link target is missing or is a directory")
            if "target" not in record:
                resolved[current] = record
                break
            chain.append(current)
            current = record["target"]
        for alias in reversed(chain):
            resolved[alias] = dict(resolved[current])
        total += resolved[name]["size"]
        if total > maximum:
            raise ValueError("upstream archive exceeds its unpacked size bound including alias copies")
    return resolved


def validate_recipe(recipe):
    required = {"schema", "component", "platform", "version", "source", "format", "root",
                "entrypoint", "extract_path", "install_path", "max_unpacked_bytes", "license"}
    optional = {"binary_path", "model_content", "notices"}
    if not isinstance(recipe, dict) or set(recipe) - required - optional or required - set(recipe):
        raise ValueError("upstream capsule recipe fields differ from the build contract")
    if recipe["schema"] != SCHEMA:
        raise ValueError("unsupported upstream capsule recipe schema")
    for field in ("component", "root"):
        if "/" in relative(recipe[field]):
            raise ValueError(f"{field} must be one portable path segment")
    if recipe["platform"] not in ("linux-amd64", "linux-arm64", "darwin-arm64", "*"):
        raise ValueError("unsupported upstream capsule platform")
    if not isinstance(recipe["version"], str) or not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[A-Za-z0-9.-]+)?", recipe["version"]):
        raise ValueError("capsule version requires a semver-like value")
    for field in ("entrypoint", "extract_path", "install_path"):
        relative(recipe[field])
    if "binary_path" in recipe:
        relative(recipe["binary_path"])
    root = recipe["root"]
    if recipe["extract_path"] != root and not recipe["extract_path"].startswith(root + "/"):
        raise ValueError("extraction path must be inside the capsule root")
    if recipe["format"] not in ("raw", "tar.gz", "zip"):
        raise ValueError("unsupported upstream payload format")
    maximum = recipe["max_unpacked_bytes"]
    if type(maximum) is not int or not 0 < maximum <= MAX_BYTES:
        raise ValueError("capsule requires a bounded unpacked size")
    source_spec(recipe["source"])
    license_info = recipe["license"]
    if (not isinstance(license_info, dict) or set(license_info) != {"spdx_id", "files"}
            or not isinstance(license_info["spdx_id"], str) or not license_info["spdx_id"].strip()
            or not isinstance(license_info["files"], list) or not license_info["files"]):
        raise ValueError("capsule requires an explicit license and pinned license files")
    notices = recipe.get("notices", [])
    if not isinstance(notices, list) or len(notices) + len(license_info["files"]) > 32:
        raise ValueError("capsule notices must be pinned build input files")
    names = set()
    for item in [*license_info["files"], *notices]:
        if not isinstance(item, dict) or set(item) != {"name", "source"}:
            raise ValueError("license/notice requires a name and pinned source")
        name = relative(item["name"])
        if name in names or name in ("capsule.json", "PROVENANCE.json", "_elastos_object.json", recipe["entrypoint"]):
            raise ValueError("duplicate or reserved license/notice file name")
        source_spec(item["source"], METADATA_BYTES)
        names.add(name)
    if "LICENSE" not in names:
        raise ValueError("capsule requires a pinned LICENSE file")
    model = recipe.get("model_content")
    if recipe["component"].startswith("model-") and model is None:
        raise ValueError("model capsule requires its explicit model_content contract")
    if model is not None:
        if recipe["format"] != "raw" or not recipe["entrypoint"].endswith(".gguf"):
            raise ValueError("model capsule requires raw GGUF weights")
        for path in [recipe["entrypoint"], *names]:
            if any(part.endswith(".") or not re.fullmatch(r"[A-Za-z0-9._-]+", part) for part in path.split("/")):
                raise ValueError("model capsule path differs from its Runtime contract")
        expected = {"format", "quantization", "engine", "consumer_interface", "consumer_interface_version",
                    "minimum_memory_mb", "license", "provenance"}
        if not isinstance(model, dict) or set(model) != expected:
            raise ValueError("model_content fields differ from the model contract")
        if (model["format"] != "gguf" or model["quantization"] not in ("Q1_0", "Q4_K_M", "Q8_0")
                or model["engine"] != "llama.cpp" or model["consumer_interface"] != "elastos.provider.model"
                or model["consumer_interface_version"] != "0.1.0"
                or type(model["minimum_memory_mb"]) is not int
                or not 0 < model["minimum_memory_mb"] <= 1_048_576):
            raise ValueError("unsupported model_content format or resource facts")
        provenance = model["provenance"]
        expected = {"base_repository", "base_revision", "base_license", "quantized_repository", "quantized_revision", "path"}
        if not isinstance(provenance, dict) or set(provenance) != expected:
            raise ValueError("model provenance fields differ from the model contract")
        for key in ("base_revision", "quantized_revision"):
            if not isinstance(provenance[key], str) or not re.fullmatch(r"[0-9a-f]{40}", provenance[key]):
                raise ValueError("model provenance requires exact Git revisions")
        for key in ("base_repository", "quantized_repository"):
            if len(relative(provenance[key]).split("/")) != 2:
                raise ValueError("model provenance requires owner/repository identities")
        for notice in (model["license"], provenance["base_license"]):
            if (not isinstance(notice, dict) or set(notice) != {"spdx_id", "path"}
                    or notice["spdx_id"] != "Apache-2.0" or notice["path"] not in names):
                raise ValueError("model license reference is absent or unsupported")
        if provenance["path"] not in names:
            raise ValueError("model provenance notice is absent")
    return recipe


def source_record(source):
    # Local build paths belong to operator state, not distributed provenance.
    return {key: value for key, value in source.items() if key != "path"}


def public_recipe(recipe):
    value = dict(recipe, source=source_record(recipe["source"]))
    value["license"] = dict(recipe["license"], files=[
        dict(item, source=source_record(item["source"])) for item in recipe["license"]["files"]])
    if "notices" in recipe:
        value["notices"] = [dict(item, source=source_record(item["source"])) for item in recipe["notices"]]
    return value


def validate_closure_paths(paths):
    """Every payload, notice and metadata member is one portable regular file."""
    canonical_paths = {}
    for path in paths:
        folded = path.lower()
        if folded in canonical_paths and canonical_paths[folded] != path:
            raise ValueError("capsule closure contains a case alias")
        canonical_paths[folded] = path
    for path in canonical_paths:
        if any(str(parent) in canonical_paths for parent in PurePosixPath(path).parents):
            raise ValueError("capsule closure places a member beneath a regular file")


def package(recipe, cache, output):
    validate_recipe(recipe)
    cache, output = directory(cache), directory(output)
    payload = cached_input(recipe["source"], cache)
    root, kind = recipe["root"], recipe["format"]
    if kind == "raw":
        members = {root + "/" + recipe["entrypoint"]: {
            "size": payload.stat().st_size, "mode": 0o644 if recipe.get("model_content") else 0o755}}
        if payload.stat().st_size > recipe["max_unpacked_bytes"]:
            raise ValueError("raw upstream payload exceeds its unpacked size bound")
    else:
        members = archive_members(payload, kind, root, recipe["max_unpacked_bytes"])
    if root + "/" + recipe["entrypoint"] not in members:
        raise ValueError("upstream archive has no recorded capsule entrypoint")
    if not any(name == recipe["extract_path"] or name.startswith(recipe["extract_path"] + "/") for name in members):
        raise ValueError("upstream archive has no recorded extraction path")
    capsule = {"schema": "elastos.capsule/v1", "name": recipe["component"],
               "version": recipe["version"], "role": "content", "type": "data",
               "projections": ["content"], "entrypoint": recipe["entrypoint"]}
    if recipe.get("model_content") is not None:
        capsule["model_content"] = recipe["model_content"]
        with payload.open("rb") as stream:
            if stream.read(4) != b"GGUF":
                raise ValueError("model payload has no GGUF header")
    # Model admission reserializes this Value with serde_json::to_vec and then
    # checks its indexed hash. Its compact JSON bytes have no final newline.
    capsule_bytes = canonical(capsule)[:-1] if recipe.get("model_content") else canonical(capsule)
    extra = {"capsule.json": capsule_bytes, "PROVENANCE.json": canonical({
        "schema": SCHEMA, "component": recipe["component"], "platform": recipe["platform"],
        "license": recipe["license"]["spdx_id"],
        "recipe_sha256": hashlib.sha256(canonical(public_recipe(recipe))).hexdigest(),
        "upstream": source_record(recipe["source"]),
        "notices": [{"name": item["name"], "source": source_record(item["source"])}
                    for item in [*recipe["license"]["files"], *recipe.get("notices", [])]]})}
    for item in [*recipe["license"]["files"], *recipe.get("notices", [])]:
        extra[item["name"]] = cached_input(item["source"], cache, METADATA_BYTES).read_bytes()
    if any(root + "/" + name in members for name in ("capsule.json", "PROVENANCE.json", "_elastos_object.json")):
        raise ValueError("upstream payload collides with capsule metadata")
    validate_closure_paths(set(members) | {root + "/" + name for name in (*extra, "_elastos_object.json")})
    if kind != "raw":
        opener = tarfile.open(payload, "r:gz") if kind == "tar.gz" else zipfile.ZipFile(payload)
        with opener as original:
            for name, data in extra.items():
                key = root + "/" + name
                if key in members:
                    stream = original.extractfile(members[key]["source"]) if kind == "tar.gz" else original.open(key)
                    with stream:
                        if members[key]["size"] != len(data) or stream.read(len(data) + 1) != data:
                            raise ValueError("upstream notice differs from its separately pinned license input")
    total = sum(item["size"] for item in members.values()) + sum(map(len, extra.values()))
    if recipe.get("model_content") and (len(members) + len(extra) > 32 or total > MAX_BYTES):
        raise ValueError("model content closure exceeds its file or total size bound")
    disk_gate(output, total + METADATA_BYTES)
    label = recipe["platform"] if recipe["platform"] != "*" else "any"
    release_path = relative(f"{recipe['component']}-{label}.tar.gz")
    destination = output / release_path
    if destination.exists() or destination.is_symlink():
        raise ValueError("capsule output already exists")
    fd, temporary = tempfile.mkstemp(prefix=".capsule-", dir=output)
    temporary = Path(temporary)
    records = []
    archive = None
    try:
        if kind != "raw":
            archive = tarfile.open(payload, "r:gz") if kind == "tar.gz" else zipfile.ZipFile(payload)
        with os.fdopen(fd, "wb") as target, gzip.GzipFile(filename="", mode="wb", fileobj=target, mtime=0) as compressed, tarfile.open(fileobj=compressed, mode="w|", format=tarfile.GNU_FORMAT) as bundle:
            def add(name, stream, size, mode):
                header = tarfile.TarInfo(root + "/" + name)
                header.size, header.mode, header.mtime = size, mode, 0
                value = hashlib.sha256()
                class Reader:
                    def read(self, count):
                        data = stream.read(count)
                        value.update(data)
                        return data
                bundle.addfile(header, Reader())
                records.append({"path": name, "sha256": value.hexdigest(), "size": size})
            for name in sorted(set(members) | {root + "/" + name for name in extra}):
                short = name[len(root) + 1:]
                if short in extra:
                    data = extra[short]
                    add(short, io.BytesIO(data), len(data), 0o644)
                else:
                    stream = (payload.open("rb") if kind == "raw" else archive.extractfile(members[name]["source"])
                              if kind == "tar.gz" else archive.open(name))
                    with stream:
                        add(short, stream, members[name]["size"], members[name]["mode"])
            records.sort(key=lambda item: item["path"])
            value = hashlib.sha256()
            for record in records:
                for field in (record["path"], record["sha256"], str(record["size"])):
                    value.update(field.encode() + b"\0")
            index = {"schema": "elastos.content.object.manifest/v1", "kind": "capsule",
                     "files": list(records), "content_digest": "sha256:" + value.hexdigest()}
            data = canonical(index)
            add("_elastos_object.json", io.BytesIO(data), len(data), 0o644)
        algorithm, expected = checksum_parts(recipe["source"]["checksum"])
        if digest(payload, algorithm) != expected:
            raise ValueError("upstream cache changed during packaging")
        os.link(temporary, destination)
        temporary.unlink()
    finally:
        if archive is not None:
            archive.close()
        temporary.unlink(missing_ok=True)
    result = {"schema": SCHEMA, "component": recipe["component"], "platform": recipe["platform"],
              "release_path": release_path, "checksum": "sha256:" + digest(destination),
              "size": destination.stat().st_size, "extract_path": recipe["extract_path"],
              "install_path": recipe["install_path"], "capsule_manifest": capsule,
              "object_manifest": index,
              "capsule_metadata": {"install_path": "capsules/" + recipe["component"],
                  "extract_path": root, "release_path": release_path}}
    if "binary_path" in recipe:
        result["binary_path"] = recipe["binary_path"]
    result["capsule_metadata"].update(checksum=result["checksum"], size=result["size"])
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--recipe", type=Path, required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if regular(args.recipe).stat().st_size > METADATA_BYTES:
        raise ValueError("upstream recipe exceeds its metadata bound")
    print(json.dumps(package(json.loads(args.recipe.read_bytes()), args.cache, args.output), sort_keys=True))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, tarfile.TarError, zipfile.BadZipFile) as error:
        sys.exit(f"upstream capsule refused: {error}")
