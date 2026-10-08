#!/usr/bin/env python3
"""Offline-custodian release signer. Install this tool; run pinned Python with -I -S.
Launch it from the signing account with an empty environment (env -i), so the
OS loader and Python start without candidate-controlled library/site settings.

Only api.github.com supplies source authority. The operator-owned policy (outside
the input root) pins repository/commit/tree/version/channel and either a
canary develop_oid or a remote version tag/tag_oid for main admission,
publisher_did, manifest_sha256 and tool/python/openssl {path, sha256}, plus a
custodian-owned key_path and max_file_bytes/max_snapshot_bytes quotas. The key
is an Ed25519 PEM readable by OpenSSL only.

Approved manifest: source {commit, tree}, version, channel, files mapping relative
paths to {sha256, size, cid}; installer {blob_oid, stamps}; release payload;
head {updated_at, prev_head_cid}. Stamps are public bootstrap inputs and exclude
HEAD_CID, which is empty to avoid the installer/release/head hash cycle.
The renderer obtains scripts/install.sh from the approved tree as inert bytes.
The release payload includes installer_sha256; artifact {cid, sha256} refs match
the snapshot records, including component checksum/release_path references.
Approved raw and UnixFS/dag-pb CIDs retain their exact spelling. File SHA-256
always binds bytes; only raw CIDs have a directly comparable file digest.
The head binds release bytes by bounded single-chunk Kubo UnixFS CIDv0 and
SHA-256. Outputs are a new read-only publication snapshot, outside input root.

This code neither builds candidates nor runs candidate tools. Production custody,
real signing and installer integration require separate operator acceptance.

The trusted signing_role defaults to release. publisher-keys accepts only
{source: {commit, tree}, statement: unsigned publisher-keys payload} and emits
one root signature in publisher-keys.json. Source authority follows the policy
channel: approved develop_oid for canary, version tag/main for other channels.
Explicit max_statement_lifetime, max_future_skew and minimum_statement_version
policy bounds apply at one fixed admission time. A rotation output is partial;
client admission, dual-signature assembly and publication are separate work.
"""

import argparse
import base64
from dataclasses import dataclass
import hashlib
import http.client
import json
import os
from pathlib import Path
import re
import shutil
import ssl
import stat
import subprocess
import sys
import tempfile
import time


REPOSITORY = "Elacity/elastos-runtime"
OID = re.compile(r"[0-9a-f]{40}\Z")
HASH = re.compile(r"[0-9a-f]{64}\Z")
VERSION = re.compile(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)(?:-(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*)(?:\.(?:0|[1-9][0-9]*|[0-9]*[A-Za-z-][0-9A-Za-z-]*))*)?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?\Z")
BASE58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
MAX_JSON = 2 * 1024 * 1024
MAX_FILE = 512 * 1024 * 1024
CHANNELS = {"stable", "canary", "jetson-test"}
STAMPS = {"MAINTAINER_DID", "SOURCE_CONNECT_TICKET", "PUBLISHER_GATEWAY", "PUBLISHER_NODE_ID", "IPNS_NAME"}
MAX_RELEASE_DIDS = 16


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def check_release_changes(changes):
    """Match operator_control.rs text bounds, including UTF-8 byte lengths."""
    require(type(changes) is list, "release changes must be a string array")
    require(len(changes) <= 32, "release has too many changes")
    total = 0
    for change in changes:
        require(type(change) is str, "release changes must be strings")
        size = len(change.encode("utf-8"))
        total += size
        require(change.strip() and size <= 500 and total <= 8 * 1024
                and not any(ord(char) < 32 or 127 <= ord(char) <= 159 for char in change),
                "release changes exceed their text bounds")


def pairs(items):
    result = {}
    for name, value in items:
        require(name not in result, "duplicate JSON field")
        result[name] = value
    return result


def json_bytes(value):
    def check(item):
        if isinstance(item, dict):
            require(all(type(k) is str for k in item), "JSON keys must be strings")
            for child in item.values():
                check(child)
        elif isinstance(item, list):
            for child in item:
                check(child)
        elif type(item) is int:
            require(-(2**63) <= item < 2**64, "JSON integer out of Rust range")
        else:
            require(item is None or type(item) in (str, bool), "JSON floats are refused")
    check(value)
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"), allow_nan=False).encode("utf-8")


def parse_json(data):
    require(len(data) <= MAX_JSON, "JSON input too large")
    value = json.loads(data, object_pairs_hook=pairs)
    json_bytes(value)
    require(type(value) is dict, "JSON object required")
    return value


def oid(value):
    require(type(value) is str and OID.fullmatch(value), "full Git SHA-1 required")
    return value


def checked_hash(value):
    require(type(value) is str and HASH.fullmatch(value), "SHA-256 required")
    return value


def public_did(raw):
    require(type(raw) is bytes and len(raw) == 32, "Ed25519 public key required")
    number = int.from_bytes(b"\xed\x01" + raw, "big")
    text = ""
    while number:
        number, digit = divmod(number, 58)
        text = BASE58[digit] + text
    return "did:key:z" + text


def base58_encode(data):
    number = int.from_bytes(data, "big")
    result = ""
    while number:
        number, digit = divmod(number, 58)
        result = BASE58[digit] + result
    return "1" * (len(data) - len(data.lstrip(b"\0"))) + result


def varint(value):
    encoded = bytearray()
    while value >= 128:
        encoded.append((value & 127) | 128)
        value >>= 7
    encoded.append(value)
    return bytes(encoded)


def cid_info(text):
    require(type(text) is str and 0 < len(text) <= 100, "supported canonical CID required")
    if text.startswith("Qm"):
        number = 0
        for char in text:
            require(char in BASE58, "CID base58 refused")
            number = number * 58 + BASE58.index(char)
        data = number.to_bytes((number.bit_length() + 7) // 8, "big")
        require(len(data) == 34 and data[:2] == b"\x12\x20" and base58_encode(data) == text, "canonical CIDv0 sha256 required")
        return 0x70, data[2:]
    require(re.fullmatch(r"b[a-z2-7]+", text), "canonical CIDv1 base32 required")
    encoded = text[1:]
    data = base64.b32decode(encoded.upper() + "=" * ((-len(encoded)) % 8))
    require("b" + base64.b32encode(data).decode().lower().rstrip("=") == text, "noncanonical CID base32")
    offset = 0
    def integer():
        nonlocal offset
        start, value, shift = offset, 0, 0
        while offset < len(data) and shift <= 63:
            byte = data[offset]
            offset += 1
            value |= (byte & 127) << shift
            if byte < 128:
                require(data[start:offset] == varint(value), "noncanonical CID varint")
                return value
            shift += 7
        raise ValueError("CID varint refused")
    version, codec, hash_code, digest_size = (integer() for _ in range(4))
    require(version == 1 and codec in (0x55, 0x70) and hash_code == 0x12 and digest_size == 32
            and len(data) - offset == 32, "supported CID codec and sha256 required")
    return codec, data[offset:]


def check_did(did):
    require(type(did) is str and re.fullmatch(r"did:key:z6Mk[1-9A-HJ-NP-Za-km-z]{44}", did), "canonical Ed25519 DID required")
    number = 0
    for char in did[len("did:key:z"):]:
        number = number * 58 + BASE58.index(char)
    encoded = number.to_bytes((number.bit_length() + 7) // 8, "big")
    require(len(encoded) == 34 and encoded[:2] == b"\xed\x01" and public_did(encoded[2:]) == did, "canonical Ed25519 DID required")


def relative_path(value):
    require(type(value) is str and len(value) <= 240 and "\\" not in value,
            "unsafe relative path")
    require(all(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", part)
                and part not in (".", "..") for part in value.split("/")), "unsafe relative path")
    return value


def protected(info):
    require(info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022,
            "trusted path has unprotected ownership or permissions")


def directory_fd(path, trusted=False):
    path = Path(path)
    require(path.is_absolute() and ".." not in path.parts, "canonical absolute path required")
    fd = os.open("/", os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        if trusted:
            protected(os.fstat(fd))
        for part in path.parts[1:]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd)
            os.close(fd)
            fd = child
            if trusted:
                protected(os.fstat(fd))
        return fd
    except Exception:
        os.close(fd)
        raise


def file_fd(path, trusted=False, root_fd=None):
    path = Path(path)
    if root_fd is None:
        parent = directory_fd(path.parent, trusted)
        parts = (path.name,)
    else:
        relative_path(str(path))
        parent = os.dup(root_fd)
        parts = path.parts
    try:
        for part in parts[:-1]:
            child = os.open(part, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=parent)
            os.close(parent)
            parent = child
        result = os.open(parts[-1], os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK, dir_fd=parent)
        if trusted:
            try:
                protected(os.fstat(result))
            except Exception:
                os.close(result)
                raise
        return result
    finally:
        os.close(parent)


def regular_bytes(path, limit, trusted=False, root_fd=None):
    fd = file_fd(path, trusted, root_fd)
    try:
        before = os.fstat(fd)
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, "regular unlinked file required")
        require(0 < before.st_size <= limit, "file size refused")
        with os.fdopen(fd, "rb", closefd=False) as source:
            data = source.read(limit + 1)
        after = os.fstat(fd)
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns), "file changed during snapshot")
        require(len(data) == before.st_size, "file size changed")
        return data
    finally:
        os.close(fd)


def github_json(path):
    # Direct HTTPS ignores environment proxies. http.client never follows redirects.
    require(not any(name in os.environ for name in ("SSL_CERT_FILE", "SSL_CERT_DIR")), "TLS environment override refused")
    require(path.startswith(f"/repos/{REPOSITORY}/git/")
            or path.startswith(f"/repos/{REPOSITORY}/compare/"), "GitHub path refused")
    connection = http.client.HTTPSConnection("api.github.com", timeout=20,
                                             context=ssl.create_default_context())
    try:
        connection.request("GET", path, headers={"Accept": "application/vnd.github+json",
                           "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "elastos-release-signer"})
        response = connection.getresponse()
        require(response.status == 200, "GitHub source verification failed")
        require(response.getheader("Content-Type", "").split(";")[0] == "application/json", "GitHub JSON required")
        return parse_json(response.read(MAX_JSON + 1))
    finally:
        connection.close()


def ref_object(response, name):
    require(response.get("ref") == "refs/" + name, "remote ref differs")
    obj = response.get("object")
    require(type(obj) is dict and obj.get("type") in ("commit", "tag"), "typed ref object required")
    oid(obj.get("sha"))
    return obj


def verify_source(policy, fetch):
    require(policy.get("repository") == REPOSITORY, "repository refused")
    version = policy.get("version")
    require(type(version) is str and VERSION.fullmatch(version) and len(version) <= 128, "release version refused")
    require(policy.get("channel") in CHANNELS, "release channel refused")
    for field in ("commit", "tree"):
        oid(policy.get(field))
    prefix = f"/repos/{REPOSITORY}"
    canary = policy["channel"] == "canary"
    if canary:
        oid(policy.get("develop_oid"))
    else:
        require(policy.get("tag") == "v" + version, "tag must match release version")
        oid(policy.get("tag_oid"))
        tag = ref_object(fetch(f"{prefix}/git/ref/tags/{policy['tag']}"), "tags/" + policy["tag"])
        require(tag["sha"] == policy["tag_oid"], "approved tag moved")
        seen = set()
        while tag["type"] == "tag":
            require(tag["sha"] not in seen and len(seen) < 4, "tag chain refused")
            seen.add(tag["sha"])
            response = fetch(f"{prefix}/git/tags/{tag['sha']}")
            require(response.get("sha") == tag["sha"] and response.get("tag") == policy["tag"], "tag object differs")
            tag = response.get("object")
            require(type(tag) is dict and tag.get("type") in ("tag", "commit"), "tag target refused")
            oid(tag.get("sha"))
        require(tag["sha"] == policy["commit"], "tag commit differs")
    commit = fetch(f"{prefix}/git/commits/{policy['commit']}")
    require(commit.get("sha") == policy["commit"] and type(commit.get("tree")) is dict
            and commit["tree"].get("sha") == policy["tree"], "approved source tree differs")
    branch = "develop" if canary else "main"
    head = ref_object(fetch(f"{prefix}/git/ref/heads/{branch}"), "heads/" + branch)
    require(head["type"] == "commit", branch + " commit required")
    if canary:
        require(head["sha"] == policy["develop_oid"], "approved develop ref moved")
    head_commit = fetch(f"{prefix}/git/commits/{head['sha']}")
    require(head_commit.get("sha") == head["sha"] and type(head_commit.get("tree")) is dict,
            "typed " + branch + " commit required")
    oid(head_commit["tree"].get("sha"))
    compare = fetch(f"{prefix}/compare/{policy['commit']}...{head['sha']}?per_page=1")
    require(compare.get("status") in ("ahead", "identical")
            and type(compare.get("behind_by")) is int and compare["behind_by"] == 0
            and type(compare.get("ahead_by")) is int and compare["ahead_by"] >= 0,
            "candidate is outside " + branch)
    require((compare["status"] == "identical") == (compare["ahead_by"] == 0)
            and (head["sha"] == policy["commit"]) == (compare["status"] == "identical"), "comparison status differs")
    for field, expected in (("base_commit", policy["commit"]), ("merge_base_commit", policy["commit"])):
        require(type(compare.get(field)) is dict and compare[field].get("sha") == expected, "comparison commit differs")


def installer_template(policy, blob_oid, fetch):
    prefix = f"/repos/{REPOSITORY}/git"
    tree_oid = policy["tree"]
    for name, kind in (("scripts", "tree"), ("install.sh", "blob")):
        response = fetch(f"{prefix}/trees/{tree_oid}")
        require(response.get("sha") == tree_oid and response.get("truncated") is False
                and type(response.get("tree")) is list and len(response["tree"]) <= 10000, "complete typed tree required")
        entries = [entry for entry in response["tree"] if type(entry) is dict and entry.get("path") == name]
        require(len(entries) == 1 and entries[0].get("type") == kind,
                "installer tree entry refused")
        require(entries[0].get("mode") in (("040000",) if kind == "tree" else ("100644", "100755")), "installer object mode refused")
        tree_oid = oid(entries[0].get("sha"))
    require(tree_oid == oid(blob_oid), "installer blob differs from approved input")
    blob = fetch(f"{prefix}/blobs/{tree_oid}")
    require(blob.get("sha") == tree_oid and blob.get("encoding") == "base64"
            and type(blob.get("size")) is int and 0 < blob["size"] <= MAX_JSON
            and type(blob.get("content")) is str, "typed installer blob required")
    data = base64.b64decode(blob["content"].replace("\n", ""), validate=True)
    require(len(data) == blob["size"] and hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == tree_oid,
            "installer Git blob hash differs")
    return data


def render_installer(template, stamps, did):
    require(type(stamps) is dict and set(stamps) == STAMPS, "exact public installer stamps required")
    require(stamps["MAINTAINER_DID"] == did, "installer publisher differs")
    require(re.fullmatch(r"https://[A-Za-z0-9.-]+(?::[0-9]{1,5})?", stamps["PUBLISHER_GATEWAY"] or ""), "frozen HTTPS gateway required")
    require(bool(stamps["SOURCE_CONNECT_TICKET"]) and bool(stamps["PUBLISHER_NODE_ID"]), "publisher delivery bootstrap required")
    text = template.decode("utf-8")
    for name, value in {**stamps, "HEAD_CID": ""}.items():
        require(type(value) is str and len(value) <= 16384
                and "__" not in value and re.fullmatch(r"[A-Za-z0-9:/?&=._+%-]*", value), "unsafe public installer stamp")
        require("__" + name + "__" in text, "installer stamp missing from template")
        text = text.replace("__" + name + "__", value)
    return text.encode("utf-8")


def raw_cid(data):
    return cid_from_hash(sha256(data))


def cid_from_hash(digest):
    return "b" + base64.b32encode(b"\x01\x55\x12\x20" + bytes.fromhex(checked_hash(digest))).decode().lower().rstrip("=")


def unixfs_metadata_cid(data):
    # Matches Runtime's single-block metadata admission and Kubo's existing
    # add_path defaults. Multi-block imports require separate importer proof.
    require(type(data) is bytes and 0 < len(data) <= 256 * 1024, "single-chunk UnixFS metadata required")
    unixfs = b"\x08\x02\x12" + varint(len(data)) + data + b"\x18" + varint(len(data))
    node = b"\x0a" + varint(len(unixfs)) + unixfs
    return base58_encode(b"\x12\x20" + hashlib.sha256(node).digest())


def snapshot_artifact(source_path, destination, record, root_fd=None):
    # Files are streamed into a private custodian directory. Candidate bytes are
    # never retained as live paths or mapped into a signing process.
    fd = file_fd(source_path, root_fd=root_fd)
    try:
        before = os.fstat(fd)
        require(stat.S_ISREG(before.st_mode) and before.st_nlink == 1, "regular unlinked file required")
        require(before.st_size == record["size"], "artifact size differs")
        destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
        digest = hashlib.sha256()
        size = 0
        with os.fdopen(fd, "rb", closefd=False) as source, destination.open("xb") as target:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                size += len(chunk)
                require(size <= record["size"], "artifact grew during snapshot")
                digest.update(chunk)
                target.write(chunk)
        after = os.fstat(fd)
        require((before.st_dev, before.st_ino, before.st_size, before.st_mtime_ns, before.st_ctime_ns)
                == (after.st_dev, after.st_ino, after.st_size, after.st_mtime_ns, after.st_ctime_ns), "artifact changed during snapshot")
        require(size == record["size"] and digest.hexdigest() == checked_hash(record["sha256"]), "artifact differs from approval")
        codec, cid_digest = cid_info(record["cid"])
        if codec == 0x55:
            require(cid_digest == digest.digest(), "artifact raw CID differs")
        destination.chmod(0o400)
    finally:
        os.close(fd)


@dataclass(frozen=True)
class Prepared:
    publisher_did: str
    files: tuple
    release: bytes
    updated_at: int
    prev_head_cid: object


@dataclass(frozen=True)
class PreparedStatement:
    publisher_did: str
    statement: bytes


def signing_source_policy(policy):
    role = policy.get("signing_role", "release")
    require(role in ("release", "publisher-keys"), "trusted signing role refused")
    # Both custody roles retain the channel-specific source admission gate.
    return policy


def prepare(policy, root, manifest_name, fetch, snapshot_root, now=None):
    source_policy = signing_source_policy(policy)
    admission_time = int(time.time()) if now is None else now
    held_root = directory_fd(root)
    try:
        if policy.get("signing_role", "release") == "publisher-keys":
            return prepare_statement(policy, source_policy, root, manifest_name, fetch,
                                     snapshot_root, held_root, admission_time)
        return prepare_from_root(policy, root, manifest_name, fetch, snapshot_root, held_root)
    finally:
        os.close(held_root)


def prepare_statement(policy, source_policy, root, manifest_name, fetch, snapshot_root, held_root, now):
    require(type(policy.get("channel")) is str and policy["channel"] in CHANNELS,
            "trusted statement channel refused")
    verify_source(source_policy, fetch)
    check_did(policy.get("publisher_did"))
    root, snapshot_root = Path(root), Path(snapshot_root)
    require(root.is_absolute() and root == root.resolve() and snapshot_root.is_absolute()
            and snapshot_root == snapshot_root.resolve() and not snapshot_root.is_relative_to(root),
            "custodian snapshot must be outside canonical input root")
    data = regular_bytes(relative_path(manifest_name), MAX_JSON, root_fd=held_root)
    require(sha256(data) == checked_hash(policy.get("manifest_sha256")), "manifest differs from operator approval")
    manifest = parse_json(data)
    require(set(manifest) == {"source", "statement"}, "publisher-keys input fields refused")
    require(manifest["source"] == {field: policy[field] for field in ("commit", "tree")},
            "statement source differs")
    statement = manifest["statement"]
    require(type(statement) is dict and set(statement) == {"schema", "version", "channel", "root_did",
            "previous_root_did", "issued_at", "expires_at", "release_dids"}, "publisher-keys fields refused")
    require(statement["schema"] == "elastos.publisher-keys/v1"
            and type(statement["channel"]) is str and statement["channel"] in CHANNELS
            and statement["channel"] == policy["channel"],
            "publisher-keys schema/channel differs")
    for field, minimum in (("max_statement_lifetime", 1), ("max_future_skew", 0), ("minimum_statement_version", 1)):
        require(type(policy.get(field)) is int and minimum <= policy[field] < 2**63,
                "explicit statement policy bound required")
    require(type(statement["version"]) is int
            and policy["minimum_statement_version"] <= statement["version"] < 2**63,
            "statement version rollback or type refused")
    require(type(now) is int and 0 <= now < 2**63, "fixed admission time required")
    issued, expires = statement["issued_at"], statement["expires_at"]
    require(type(issued) is int and type(expires) is int and 0 <= issued < expires < 2**63,
            "statement timestamps refused")
    require(expires > now, "expired publisher-keys statement refused")
    require(issued <= now + policy["max_future_skew"], "future publisher-keys statement refused")
    require(expires - issued <= policy["max_statement_lifetime"], "statement lifetime refused")
    roots = [statement["root_did"]]
    check_did(roots[0])
    previous = statement["previous_root_did"]
    if previous is not None:
        check_did(previous)
        require(previous != roots[0], "distinct rotation roots required")
        roots.append(previous)
    require(policy["publisher_did"] in roots, "approved signer is outside statement roots")
    delegates = statement["release_dids"]
    require(type(delegates) is list and len(delegates) <= MAX_RELEASE_DIDS,
            "bounded release DIDs required")
    for did in delegates:
        check_did(did)
        require(did not in roots, "root cannot be a release delegate")
    require(len(set(delegates)) == len(delegates), "distinct release DIDs required")
    for quota in ("max_file_bytes", "max_snapshot_bytes"):
        require(type(policy.get(quota)) is int and 0 < policy[quota] < 2**63, "trusted snapshot quota required")
    payload = json_bytes(statement)
    require(len(data) <= policy["max_file_bytes"] and len(payload) + 300 <= policy["max_snapshot_bytes"]
            and len(payload) + 300 <= 256 * 1024, "statement snapshot quota refused")
    usage = shutil.disk_usage(snapshot_root)
    require((usage.free - 3 * MAX_JSON) * 100 >= usage.total * 15,
            "snapshot would cross the 15 percent free-space floor")
    return PreparedStatement(policy["publisher_did"], payload)


def prepare_from_root(policy, root, manifest_name, fetch, snapshot_root, held_root):
    verify_source(policy, fetch)
    check_did(policy.get("publisher_did"))
    root = Path(root)
    snapshot_root = Path(snapshot_root)
    require(root.is_absolute() and root == root.resolve() and snapshot_root.is_absolute()
            and snapshot_root == snapshot_root.resolve() and not snapshot_root.is_relative_to(root),
            "custodian snapshot must be outside canonical input root")
    manifest_data = regular_bytes(relative_path(manifest_name), MAX_JSON, root_fd=held_root)
    require(sha256(manifest_data) == checked_hash(policy.get("manifest_sha256")), "manifest differs from operator approval")
    manifest = parse_json(manifest_data)
    require(set(manifest) == {"source", "version", "channel", "files", "installer", "release", "head"}, "signing input fields refused")
    source = {field: policy[field] for field in ("commit", "tree")}
    require(manifest["source"] == source and manifest["version"] == policy["version"] and manifest["channel"] == policy["channel"], "input source/version/channel differs")
    records = manifest["files"]
    require(type(records) is dict and 0 < len(records) <= 512, "bounded artifact records required")
    snapshot = []
    total = 0
    for quota in ("max_file_bytes", "max_snapshot_bytes"):
        require(type(policy.get(quota)) is int and 0 < policy[quota] < 2**63, "trusted snapshot quota required")
    for name, record in sorted(records.items()):
        relative_path(name)
        require(name not in (manifest_name, "install.sh", "release.json", "release-head.json"), "reserved artifact name")
        require(type(record) is dict and set(record) == {"sha256", "size", "cid"}, "typed artifact record required")
        checked_hash(record["sha256"])
        cid_info(record["cid"])
        require(type(record["size"]) is int and 0 < record["size"] <= policy["max_file_bytes"], "artifact size refused")
        total += record["size"]
        require(total <= policy["max_snapshot_bytes"], "publication snapshot too large")
    usage = shutil.disk_usage(snapshot_root)
    require(usage.free >= total + 3 * MAX_JSON,
            "snapshot needs more free space than the volume has")
    for name, record in sorted(records.items()):
        destination = snapshot_root / name
        snapshot_artifact(Path(name), destination, record, root_fd=held_root)
        snapshot.append((name, destination))
    installer = manifest["installer"]
    require(type(installer) is dict and set(installer) == {"blob_oid", "stamps"}, "installer input refused")
    rendered = render_installer(installer_template(policy, installer["blob_oid"], fetch), installer["stamps"], policy["publisher_did"])
    release = manifest["release"]
    require(type(release) is dict and set(release) - {"changes"} == {"schema", "source", "version", "channel", "released_at", "prev_release_cid", "platforms", "installer_sha256"}, "release fields refused")
    if "changes" in release:
        check_release_changes(release["changes"])
    require(release["schema"] == "elastos.release/v1" and release["source"] == source
            and release["version"] == policy["version"] and release["channel"] == policy["channel"]
            and type(release["released_at"]) is int and release["released_at"] >= 0
            and release["installer_sha256"] == sha256(rendered), "release identity/installer differs")
    require(release["prev_release_cid"] is None or type(release["prev_release_cid"]) is str, "previous release pointer refused")
    if release["prev_release_cid"] is not None:
        cid_info(release["prev_release_cid"])
    platforms = release["platforms"]
    require(type(platforms) is dict and platforms and set(platforms) <= {"x86_64-linux", "aarch64-linux", "aarch64-darwin"}, "release platforms refused")
    advertised = set()
    approved_refs = {(record["cid"], record["sha256"]) for record in records.values()}
    def admit_ref(cid, digest, size=None, release_path=None):
        cid_info(cid)
        checked_hash(digest)
        pair = (cid, digest)
        require(pair in approved_refs, "artifact reference differs from snapshot")
        matching = [(name, record) for name, record in records.items()
                    if (record["cid"], record["sha256"]) == pair]
        if size is not None:
            require(type(size) is int and any(record["size"] == size for _, record in matching), "artifact descriptor size differs")
        if release_path is not None:
            relative_path(release_path)
            require(release_path in records and (records[release_path]["cid"], records[release_path]["sha256"]) == pair
                    and (size is None or records[release_path]["size"] == size), "component release_path differs")
        advertised.add(pair)
    def refs(value):
        require(type(value) is dict, "artifact descriptor object required")
        if "cid" in value or "sha256" in value:
            require(set(value) in ({"cid", "sha256"}, {"cid", "sha256", "size"}), "artifact descriptor fields refused")
            require("size" not in value or type(value["size"]) is int, "artifact descriptor size refused")
            admit_ref(value["cid"], value["sha256"], value.get("size"))
        else:
            require(value, "empty descriptor refused")
            for child in value.values():
                refs(child)
    for platform in platforms.values():
        require(type(platform) is dict and set(platform) == {"binary", "components"}, "platform fields refused")
        refs(platform)
    # Component/capsule records must also be bound by a snapshotted JSON manifest.
    visited = set()
    while True:
        reachable = [(name, data) for name, data in snapshot if name.endswith(".json")
                     and name not in visited and (records[name]["cid"], records[name]["sha256"]) in advertised]
        if not reachable:
            break
        for name, path in reachable:
            visited.add(name)
            component = parse_json(regular_bytes(path, MAX_JSON))
            if component.get("schema") != "elastos.components/v1":
                continue
            def component_refs(value):
                if isinstance(value, dict):
                    if "cid" in value:
                        digest = value.get("sha256")
                        if "checksum" in value:
                            checksum = value["checksum"]
                            require(type(checksum) is str and checksum.startswith("sha256:"), "component checksum refused")
                            digest = checksum[len("sha256:"):]
                            require("sha256" not in value or value["sha256"] == digest, "component checksum differs")
                            require("release_path" in value and type(value["release_path"]) is str, "component release_path required")
                        require("size" in value, "component descriptor size required")
                        require(type(value["size"]) is int and ("release_path" not in value or type(value["release_path"]) is str), "component descriptor type refused")
                        admit_ref(value["cid"], digest, value["size"], value.get("release_path"))
                    for child in value.values():
                        component_refs(child)
                elif isinstance(value, list):
                    for child in value:
                        component_refs(child)
            component_refs(component)
            catalog = component.get("model_catalog")
            if catalog is not None:
                require(type(catalog) is dict and type(catalog.get("head_cid")) is str, "model catalog pin refused")
                record = records.get("model-catalog.json")
                require(type(record) is dict, "model catalog snapshot required")
                catalog_bytes = regular_bytes(snapshot_root / "model-catalog.json", MAX_JSON)
                require(catalog["head_cid"] == raw_cid(catalog_bytes), "model catalog head differs from snapshot")
                admit_ref(record["cid"], record["sha256"], record["size"], "model-catalog.json")
    require(advertised == approved_refs, "unbound artifact in approved snapshot")
    head = manifest["head"]
    require(type(head) is dict and set(head) == {"updated_at", "prev_head_cid"}
            and type(head["updated_at"]) is int and head["updated_at"] >= 0
            and (head["prev_head_cid"] is None or type(head["prev_head_cid"]) is str), "head input refused")
    if head["prev_head_cid"] is not None:
        cid_info(head["prev_head_cid"])
    install_path = snapshot_root / "install.sh"
    with install_path.open("xb") as output:
        output.write(rendered)
    install_path.chmod(0o400)
    snapshot.append(("install.sh", install_path))
    return Prepared(policy["publisher_did"], tuple(snapshot), json_bytes(release), head["updated_at"], head["prev_head_cid"])


def signature_digest(domain, payload):
    require(domain in ("elastos.release.v1", "elastos.release.head.v1", "elastos.publisher.keys.v1"), "signing domain refused")
    return hashlib.sha256(domain.encode() + b"\0" + payload).digest()


def sign_publication(prepared, backend):
    if isinstance(prepared, PreparedStatement):
        require(public_did(backend.public_key()) == prepared.publisher_did, "custodian public DID differs")
        digest = signature_digest("elastos.publisher.keys.v1", prepared.statement)
        signature = backend.sign(digest)
        require(type(signature) is bytes and len(signature) == 64, "Ed25519 signature length differs")
        require(backend.verify(digest, signature) is True, "signature public verification failed")
        # A rotation output contains this custodian's one signature. It is a
        # partial handover until an independently verified second root signs.
        statement = json_bytes({"payload": parse_json(prepared.statement), "signatures": [
            {"signer_did": prepared.publisher_did, "signature": signature.hex()}]})
        return (("publisher-keys.json", statement),)
    require(len(prepared.release) + 300 <= 256 * 1024, "single-chunk release metadata required")
    require(public_did(backend.public_key()) == prepared.publisher_did, "custodian public DID differs")
    def envelope(domain, payload):
        canonical = json_bytes(payload)
        digest = signature_digest(domain, canonical)
        signature = backend.sign(digest)
        require(type(signature) is bytes and len(signature) == 64, "Ed25519 signature length differs")
        require(backend.verify(digest, signature) is True, "signature public verification failed")
        return json_bytes({"payload": payload, "signature": signature.hex(), "signer_did": prepared.publisher_did})
    release = envelope("elastos.release.v1", parse_json(prepared.release))
    head = {"schema": "elastos.release.head/v1", "channel": parse_json(prepared.release)["channel"],
            "version": parse_json(prepared.release)["version"], "latest_release_cid": unixfs_metadata_cid(release),
            "release_sha256": sha256(release), "updated_at": prepared.updated_at,
            "signer_did": prepared.publisher_did, "prev_head_cid": prepared.prev_head_cid}
    return prepared.files + (("release.json", release), ("release-head.json", envelope("elastos.release.head.v1", head)))


def pinned_tools(policy, input_root):
    for name, actual in (("tool", Path(__file__).absolute()), ("python", Path(sys.executable).resolve()), ("openssl", None)):
        pin = policy.get(name)
        require(type(pin) is dict and set(pin) == {"path", "sha256"}, "executable pin required")
        path = Path(pin["path"])
        require(path.is_absolute() and path == path.resolve() and not path.is_relative_to(input_root), "trusted executable must be outside input root")
        require(actual is None or path == actual, "running tool/interpreter differs from pin")
        require(sha256(regular_bytes(path, MAX_FILE, trusted=True)) == checked_hash(pin["sha256"]), "trusted executable hash differs")


class OpenSSLBackend:
    def __init__(self, policy, input_root, temp_parent):
        self.executable = policy["openssl"]["path"]
        self.key_path = Path(policy["key_path"])
        require(self.key_path.is_absolute() and self.key_path == self.key_path.resolve() and not self.key_path.is_relative_to(input_root), "custodian key must be outside input root")
        for path in (*self.key_path.parents, self.key_path):
            info = path.lstat()
            require(not stat.S_ISLNK(info.st_mode) and info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022, "custodian key path protection refused")
        info = self.key_path.stat()
        require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1 and info.st_uid == os.getuid() and not info.st_mode & 0o077, "custodian key ownership/permissions refused")
        # Key bytes stay in OpenSSL. No Python key read or key creation exists.
        require(Path(temp_parent).is_absolute() and not Path(temp_parent).is_relative_to(input_root), "custodian temporary parent refused")
        parent_fd = directory_fd(temp_parent, trusted=True)
        os.close(parent_fd)
        self.temp = tempfile.TemporaryDirectory(prefix="elastos-signer-", dir=temp_parent)
        self.root = Path(self.temp.name)
        try:
            require(re.match(rb"OpenSSL 3\.[0-9]+\.", self.run(["version"])), "pinned OpenSSL 3 required")
            der = self.run(["pkey", "-provider", "default", "-in", str(self.key_path), "-pubout", "-outform", "DER"])
            require(len(der) == 44 and der[:12] == bytes.fromhex("302a300506032b6570032100"), "Ed25519 public DER required")
            self.public = der[12:]
            (self.root / "public.der").write_bytes(der)
        except Exception:
            self.close()
            raise

    def run(self, args):
        result = subprocess.run([self.executable, *args], cwd=self.root, env={"OPENSSL_CONF": "/dev/null", "LANG": "C"},
                                shell=False, stdin=subprocess.DEVNULL, capture_output=True, timeout=20, check=False)
        require(result.returncode == 0 and len(result.stdout) <= MAX_JSON and len(result.stderr) <= MAX_JSON, "pinned OpenSSL operation failed")
        return result.stdout

    def public_key(self):
        return self.public

    def sign(self, digest):
        (self.root / "digest").write_bytes(digest)
        return self.run(["pkeyutl", "-provider", "default", "-sign", "-rawin", "-inkey", str(self.key_path), "-in", "digest"])

    def verify(self, digest, signature):
        (self.root / "digest").write_bytes(digest)
        (self.root / "signature").write_bytes(signature)
        self.run(["pkeyutl", "-provider", "default", "-verify", "-rawin", "-pubin", "-keyform", "DER", "-inkey", "public.der", "-in", "digest", "-sigfile", "signature"])
        return True

    def close(self):
        self.temp.cleanup()


def confirmed(prepared, input_stream, output_stream):
    subject = "publisher-keys statement (one root signature)" if isinstance(prepared, PreparedStatement) else "release"
    output_stream.write(f"Sign approved {subject} as {prepared.publisher_did}.\nType this complete DID to confirm, or press Enter to cancel: ")
    output_stream.flush()
    return input_stream.readline().strip() == prepared.publisher_did


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--input-root", required=True, type=Path)
    parser.add_argument("--manifest", default="signing-input.json")
    parser.add_argument("--output-root", required=True, type=Path)
    args = parser.parse_args()
    require(sys.flags.isolated == 1 and sys.flags.no_site == 1, "run the pinned interpreter with -I -S")
    require(args.input_root.is_absolute() and args.policy.is_absolute() and args.output_root.is_absolute(), "absolute input/policy/output paths required")
    require(args.input_root == args.input_root.resolve() and args.policy == args.policy.resolve()
            and args.output_root == args.output_root.resolve(), "canonical custodian/input paths required")
    require(not args.policy.is_relative_to(args.input_root) and not args.output_root.is_relative_to(args.input_root), "custodian paths must be outside input root")
    policy = parse_json(regular_bytes(args.policy, MAX_JSON, trusted=True))
    pinned_tools(policy, args.input_root)
    require(not args.output_root.exists(), "output root already exists")
    for path in (args.output_root.parent, *args.output_root.parent.parents):
        info = path.stat()
        require(info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022,
                "output parent protection refused")
    with tempfile.TemporaryDirectory(prefix=".elastos-signing-", dir=args.output_root.parent) as scratch:
        prepared = prepare(policy, args.input_root, args.manifest, github_json, Path(scratch))
        require(confirmed(prepared, sys.stdin, sys.stderr), "signing cancelled")
        # Recheck canonical source authority after confirmation, before backend use.
        verify_source(signing_source_policy(policy), github_json)
        backend = OpenSSLBackend(policy, args.input_root, Path(scratch))
        try:
            publication = sign_publication(prepared, backend)
        finally:
            backend.close()
        args.output_root.mkdir(mode=0o700)
        try:
            for name, data in publication:
                destination = args.output_root / name
                destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
                if isinstance(data, Path):
                    data.rename(destination)
                else:
                    with destination.open("xb") as output:
                        output.write(data)
                destination.chmod(0o444)
        except Exception:
            shutil.rmtree(args.output_root)
            raise
    print("Signed approved publisher-keys statement with one root signature." if isinstance(prepared, PreparedStatement)
          else "Signed approved publication snapshot.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        sys.exit(f"Release signing refused: {error}")
