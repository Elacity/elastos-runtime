#!/usr/bin/env python3
"""Offline-custodian release signer. Install this tool; run pinned Python with -I -S.
Launch it from the signing account with an empty environment (env -i), so the
OS loader and Python start without candidate-controlled library/site settings.

Only api.github.com supplies source authority, authenticated with GH_TOKEN when
the launch passes one (env -i GH_TOKEN=...), else anonymously. Network, outage
and rate-limit failures refuse with a distinct retry message. The operator-owned
policy (outside the input root) pins repository/commit/tree/version/channel and
either a canary develop_oid or a remote version tag/tag_oid for main admission
(main must contain the tagged commit and carry exactly its tree),
publisher_did, manifest_sha256 and tool/python/openssl {path, sha256}, plus a
custodian-owned key_path and max_file_bytes/max_snapshot_bytes quotas. The key
is an Ed25519 PEM readable by OpenSSL only.

Release policies also pin a pre-existing independent approval by path/SHA-256.
It approves source H, complete workflow commit/tree W on refs/heads/develop, the
publisher delivery tuple, and an installed gh verifier by path/SHA-256. W and H
may differ. The signer verifies immutable certificate/run identities and attested
receipt digests before freezing the native payloads. W's release-package.yml must
equal develop's. Generated components-<platform>.json files must equal the
manifest re-derived from the approved source template and each platform's
attested components.json; every other file must be attested bytes. Old packages
need a new build.

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

--model-catalog PAYLOAD signs a model catalogue payload (schema, published_at,
optional expires_at, entries) from the input root with the same policy key, tool
pins and typed-DID confirmation. The policy approves the exact payload bytes by
model_catalog_sha256. It refuses payloads the Runtime would refuse before any key
use, writes canonical model-catalog.json and prints its raw CID (model_catalog.head_cid).

This code neither builds candidates nor runs candidate tools. Production custody,
real signing and installer integration require separate operator acceptance.

The explicit community-profile operation signs one approved initial Community
profile before its startup-file CID enters a release manifest. Its separate
policy pins the canonical payload and grant hashes, network, conversation,
revision and signer set. The existing maintainer PEM stays with OpenSSL.
"""

import argparse
import base64
import copy
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
# Manifest pins that name one release file by the raw CID of its exact bytes.
PINNED_RELEASE_FILES = (("model_catalog", "model-catalog.json", "model catalog"),
                        ("collaboration_network", "collaboration-network-release-v1.json", "Community network"))
STAMPS = {"MAINTAINER_DID", "SOURCE_CONNECT_TICKET", "PUBLISHER_GATEWAY", "PUBLISHER_NODE_ID", "IPNS_NAME"}
PACKAGE_WORKFLOW = ".github/workflows/release-package.yml"
PACKAGE_REF = "refs/heads/develop"
APPROVAL_SCHEMA = "elastos.release.approval/v1"
PROVENANCE_SCHEMA = "elastos.release.provenance/v1"
COMMUNITY_PROFILE_DOMAIN = "elastos.collaboration-network.profile.v1"
COMMUNITY_PROFILE_SCHEMA = "elastos.collaboration-network.profile/v1"
COMMUNITY_INPUT_SCHEMA = "elastos.collaboration-network.signing-input/v1"
COMMUNITY_STARTUP_SCHEMA = "elastos.collaboration-network.startup-config/v1"
COMMUNITY_GRANT_SCHEMA = "elastos.collaboration.default-conversation-grant/v1"
MAX_COMMUNITY_PROFILE_BYTES = 64 * 1024
MAX_COMMUNITY_GRANT_BYTES = 8 * 1024
MODEL_CATALOG_DOMAIN = "elastos.model.catalog.v1"
MODEL_CATALOG_SCHEMA = "elastos.model.catalog/v1"
MAX_MODEL_CATALOG = 128 * 1024
CAPSULE_RECORD_DOMAIN = "elastos.capsule.record.v1"
CAPSULE_RECORD_SCHEMA = "elastos.capsule.record/v1"
# A release is one UnixFS chunk; Runtime refuses larger release metadata.
MAX_RELEASE_METADATA = 256 * 1024


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


UNAVAILABLE = "GitHub or Sigstore is unavailable (network, outage or rate limit); retry later. This is not a provenance refusal"
# gh reports transport failures in these families (Go net/http, TLS, DNS) and
# GitHub outages or limits as HTTP 429/5xx or "rate limit".
UNAVAILABLE_OUTPUT = re.compile(rb"error connecting to|dial tcp|no such host|lookup [^ ]+ on|i/o timeout|timed? ?out|deadline exceeded"
                                rb"|Client\.Timeout|connection (?:refused|reset)|broken pipe|unexpected EOF|TLS handshake"
                                rb"|network is (?:unreachable|down)|temporary failure|rate limit|HTTP (?:429|5[0-9][0-9])", re.I)
# A GitHub answer that refuses the request itself (HTTP 4xx other than 429).
HTTP_REFUSAL = re.compile(rb"HTTP 4(?!29)[0-9][0-9]")


def unavailable(detail):
    return ValueError(f"{UNAVAILABLE}: {detail}")


def github_json(path):
    # Direct HTTPS ignores environment proxies. http.client never follows redirects.
    require(not any(name in os.environ for name in ("SSL_CERT_FILE", "SSL_CERT_DIR")), "TLS environment override refused")
    require(path.startswith(f"/repos/{REPOSITORY}/git/")
            or path.startswith(f"/repos/{REPOSITORY}/compare/")
            or re.fullmatch(rf"/repos/{REPOSITORY}/actions/runs/[1-9][0-9]*(?:/artifacts\?per_page=100&page=[1-9][0-9]*)?", path),
            "GitHub path refused")
    headers = {"Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28", "User-Agent": "elastos-release-signer"}
    if os.environ.get("GH_TOKEN"):
        headers["Authorization"] = "Bearer " + os.environ["GH_TOKEN"]
    connection = http.client.HTTPSConnection("api.github.com", timeout=20,
                                             context=ssl.create_default_context())
    try:
        try:
            connection.request("GET", path, headers=headers)
            response = connection.getresponse()
        except (OSError, http.client.HTTPException) as error:
            if isinstance(error, ssl.SSLCertVerificationError):
                raise
            raise unavailable(f"{type(error).__name__}: {error}") from error
        limited = response.status == 403 and (response.getheader("X-RateLimit-Remaining") == "0" or response.getheader("Retry-After"))
        if limited or response.status == 429 or response.status >= 500:
            raise unavailable(f"HTTP {response.status}")
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
    if not canary:
        # A stable release is main itself: main carries exactly the approved tree.
        require(head_commit["tree"]["sha"] == policy["tree"], "main tree differs from the approved source tree")
    compare = fetch(f"{prefix}/compare/{policy['commit']}...{head['sha']}?per_page=1")
    require(compare.get("status") in ("ahead", "identical")
            and type(compare.get("behind_by")) is int and compare["behind_by"] == 0
            and type(compare.get("ahead_by")) is int and compare["ahead_by"] >= 0,
            "candidate is outside " + branch)
    require((compare["status"] == "identical") == (compare["ahead_by"] == 0)
            and (head["sha"] == policy["commit"]) == (compare["status"] == "identical"), "comparison status differs")
    for field, expected in (("base_commit", policy["commit"]), ("merge_base_commit", policy["commit"])):
        require(type(compare.get(field)) is dict and compare[field].get("sha") == expected, "comparison commit differs")


def tree_blob(tree_oid, path, fetch):
    names = path.split("/")
    for index, name in enumerate(names):
        kind = "blob" if index == len(names) - 1 else "tree"
        response = fetch(f"/repos/{REPOSITORY}/git/trees/{tree_oid}")
        require(response.get("sha") == tree_oid and response.get("truncated") is False
                and type(response.get("tree")) is list and len(response["tree"]) <= 10000, "complete typed tree required")
        entries = [entry for entry in response["tree"] if type(entry) is dict and entry.get("path") == name]
        require(len(entries) == 1 and entries[0].get("type") == kind, path + " tree entry refused")
        require(entries[0].get("mode") in (("040000",) if kind == "tree" else ("100644", "100755")), path + " object mode refused")
        tree_oid = oid(entries[0].get("sha"))
    return tree_oid


def git_blob(blob_oid, fetch):
    blob = fetch(f"/repos/{REPOSITORY}/git/blobs/{blob_oid}")
    require(blob.get("sha") == blob_oid and blob.get("encoding") == "base64"
            and type(blob.get("size")) is int and 0 < blob["size"] <= MAX_JSON
            and type(blob.get("content")) is str, "typed Git blob required")
    data = base64.b64decode(blob["content"].replace("\n", ""), validate=True)
    require(len(data) == blob["size"] and hashlib.sha1(b"blob " + str(len(data)).encode() + b"\0" + data).hexdigest() == blob_oid,
            "Git blob hash differs")
    return data


def installer_template(policy, blob_oid, fetch):
    tree_oid = tree_blob(policy["tree"], "scripts/install.sh", fetch)
    require(tree_oid == oid(blob_oid), "installer blob differs from approved input")
    return git_blob(tree_oid, fetch)


# Release platform -> components.json platform keys, searched before "*".
COMPONENT_KEYS = {"x86_64-linux": ("linux-amd64", "x86_64-linux"),
                  "aarch64-linux": ("linux-arm64", "aarch64-linux"),
                  "aarch64-darwin": ("darwin-arm64", "macos-arm64")}


def merge_components(template, manifests):
    """The release components manifest: the source template plus each
    platform's attested descriptors, limited to the admitted platforms."""
    require(type(template) is dict and type(template.get("external")) is dict
            and all(type(component) is dict for component in template["external"].values()), "components template refused")
    merged = copy.deepcopy(template)
    merged["schema"], merged["capsules"] = "elastos.components/v1", {}
    selected, pins = {}, []
    for platform, manifest in sorted(manifests.items()):
        require(platform in COMPONENT_KEYS and type(manifest) is dict and type(manifest.get("external")) is dict,
                "platform components refused")
        pins.append(manifest.get("model_catalog"))
        for name, component in manifest["external"].items():
            require(type(component) is dict and name in merged["external"], "platform component outside template: " + str(name))
            for metadata in (False, True):
                entry = component.get("capsule_metadata") if metadata else component
                if entry is None:
                    continue
                require(type(entry) is dict and type(entry.get("platforms", {})) is dict, "platform component refused: " + name)
                key, info = next(((key, entry["platforms"][key]) for key in (*COMPONENT_KEYS[platform], "*")
                                  if type(entry.get("platforms", {}).get(key)) is dict), (None, None))
                if info is None:
                    continue
                require(selected.setdefault((name, metadata, key), info) == info, f"conflicting universal descriptor: {name} ({key})")
                target = merged["external"][name]
                if metadata:
                    contract = {k: v for k, v in entry.items() if k != "platforms"}
                    target = target.setdefault("capsule_metadata", {**contract, "platforms": {}})
                    require({k: v for k, v in target.items() if k != "platforms"} == contract,
                            "conflicting provider metadata contract: " + name)
                target.setdefault("platforms", {})[key] = copy.deepcopy(info)
    require(pins and all(pin == pins[0] for pin in pins), "platform model catalogue pins differ")
    if pins[0] is None:
        merged.pop("model_catalog", None)
    else:
        merged["model_catalog"] = copy.deepcopy(pins[0])
    # A publication advertises only the platforms it admitted.
    unselected = {keys[0] for platform, keys in COMPONENT_KEYS.items() if platform not in manifests}
    for component in merged["external"].values():
        for entry in (component, component.get("capsule_metadata")):
            if isinstance(entry, dict):
                for setup in unselected:
                    entry.get("platforms", {}).pop(setup, None)
    return merged


def bind_component_cids(manifest, cids):
    manifest = copy.deepcopy(manifest)
    for component in manifest["external"].values():
        for entry in (component, component.get("capsule_metadata", {})):
            for info in entry.get("platforms", {}).values():
                if info.get("release_path"):
                    require(info["release_path"] in cids, "component release file is not admitted: " + str(info["release_path"]))
                    info["cid"] = cids[info["release_path"]]
    return manifest


def capsule_record_payloads(manifest, publisher_did, release_version):
    """One `elastos.capsule.record/v1` payload per released row of an
    `elastos.components/v1` manifest whose CIDs are bound: each component's own
    platform rows and its capsule metadata rows. The payload names only the
    capsule's bytes and declared version, never the release version, so it stays
    byte-identical while the capsule is unchanged. Sorted by (name, release_path)."""
    require(type(manifest) is dict and type(manifest.get("external")) is dict, "components refused")
    records = {}
    for name, component in manifest["external"].items():
        require(type(name) is str and name and type(component) is dict, "component refused")
        version = component.get("version")
        require(version is None or (type(version) is str and version), "capsule version refused: " + name)
        require(version != release_version, "capsule version must not be the release version: " + name)
        for entry in (component, component.get("capsule_metadata")):
            if entry is None:
                continue
            require(type(entry) is dict and type(entry.get("platforms", {})) is dict, "component refused: " + name)
            for platform, info in entry.get("platforms", {}).items():
                require(type(info) is dict, "component row refused: " + name)
                if not info.get("release_path"):
                    continue
                checksum = info.get("checksum")
                require(type(checksum) is str and checksum.startswith("sha256:"), "capsule record checksum refused: " + name)
                require(type(info.get("size")) is int and info["size"] > 0, "capsule record size refused: " + name)
                require(type(info.get("cid")) is str, "capsule record CID refused: " + name)
                cid_info(info["cid"])
                payload = {"schema": CAPSULE_RECORD_SCHEMA, "name": name, "platform": platform,
                           "capsule_version": version, "sha256": checked_hash(checksum[len("sha256:"):]),
                           "size": info["size"], "cid": info["cid"], "release_path": relative_path(info["release_path"]),
                           "signer_did": publisher_did}
                key = (name, payload["release_path"])
                require(records.setdefault(key, payload) == payload, f"conflicting capsule record: {name} ({payload['release_path']})")
    return tuple(records[key] for key in sorted(records))


def approved_release(path, input_root=None, expected_hash=None):
    path = Path(path)
    require(path.is_absolute() and path == path.resolve(), "approval requires a canonical absolute path")
    require(input_root is None or not path.is_relative_to(input_root), "approval must be outside candidate inputs")
    data = regular_bytes(path, MAX_JSON, trusted=True)
    require(expected_hash is None or sha256(data) == checked_hash(expected_hash), "independent approval changed")
    approval = parse_json(data)
    require(type(approval) is dict and set(approval) == {"schema", "repository", "source", "workflow", "delivery", "gh"}
            and approval["schema"] == APPROVAL_SCHEMA and approval["repository"] == REPOSITORY,
            "independent release approval required")
    require(type(approval["source"]) is dict and set(approval["source"]) == {"commit", "tree"}, "approved source required")
    require(type(approval["workflow"]) is dict and set(approval["workflow"]) == {"commit", "tree", "ref"}, "approved producer required")
    for identity in (approval["source"], approval["workflow"]):
        oid(identity["commit"])
        oid(identity["tree"])
    require(approval["workflow"]["ref"] == PACKAGE_REF, "approved develop producer ref required")
    validate_delivery(approval["delivery"])
    pin = approval["gh"]
    require(type(pin) is dict and set(pin) == {"path", "sha256"}, "trusted attestation verifier required")
    executable = Path(pin["path"])
    require(executable.is_absolute() and executable == executable.resolve()
            and (input_root is None or not executable.is_relative_to(input_root)), "trusted verifier must be outside inputs")
    require(sha256(regular_bytes(executable, MAX_FILE, trusted=True)) == checked_hash(pin["sha256"]), "attestation verifier pin differs")
    return approval, sha256(data)


def validate_delivery(stamps):
    require(type(stamps) is dict and set(stamps) == STAMPS, "exact approved delivery tuple required")
    check_did(stamps["MAINTAINER_DID"])
    require(re.fullmatch(r"https://[A-Za-z0-9.-]+(?::[0-9]{1,5})?", stamps["PUBLISHER_GATEWAY"] or ""), "approved HTTPS gateway required")
    node = stamps["PUBLISHER_NODE_ID"]
    require(type(node) is str and re.fullmatch(r"[0-9a-f]{64}", node), "approved Carrier node required")
    ticket = stamps["SOURCE_CONNECT_TICKET"]
    require(type(ticket) is str and re.fullmatch(r"[a-z2-7]{1,16384}", ticket), "approved Carrier ticket required")
    try:
        ticket_data = parse_json(base64.b32decode(ticket.upper() + "=" * (-len(ticket) % 8)))
    except (ValueError, base64.binascii.Error) as error:
        raise ValueError("approved Carrier ticket is invalid") from error
    endpoints = ticket_data.get("endpoints") if type(ticket_data) is dict else None
    require(type(endpoints) is list and endpoints and len(endpoints) <= 64
            and all(type(endpoint) is dict and endpoint.get("id") == node for endpoint in endpoints),
            "every approved ticket endpoint must match the publisher node")
    require(type(stamps["IPNS_NAME"]) is str and re.fullmatch(r"[A-Za-z0-9._-]{0,240}", stamps["IPNS_NAME"]), "approved IPNS value required")


def checked_package_run(approval, run_id, fetch):
    require(type(run_id) is int and run_id > 0, "package run id required")
    workflow = approval["workflow"]
    prefix = f"/repos/{REPOSITORY}"
    run = fetch(f"{prefix}/actions/runs/{run_id}")
    require(type(run) is dict and run.get("id") == run_id
            and run.get("path") == PACKAGE_WORKFLOW and run.get("event") == "workflow_dispatch"
            and run.get("status") == "completed" and run.get("conclusion") == "success"
            and run.get("head_sha") == workflow["commit"] and "refs/heads/" + str(run.get("head_branch")) == workflow["ref"]
            and type(run.get("repository")) is dict and run["repository"].get("full_name") == REPOSITORY
            and type(run["repository"].get("id")) is int and run["repository"]["id"] > 0
            and type(run.get("head_repository")) is dict and run["head_repository"].get("full_name") == REPOSITORY
            and run["head_repository"].get("id") == run["repository"]["id"],
            "package run differs from independently approved producer")
    require(type(run.get("run_attempt")) is int and run["run_attempt"] > 0, "package run attempt required")
    for identity in (approval["workflow"], approval["source"]):
        commit = fetch(f"{prefix}/git/commits/{identity['commit']}")
        require(commit.get("sha") == identity["commit"] and commit.get("tree", {}).get("sha") == identity["tree"],
                "approved producer/source tree differs")
    develop = ref_object(fetch(f"{prefix}/git/ref/heads/develop"), "heads/develop")
    require(develop["type"] == "commit", "develop commit required")
    develop_commit = fetch(f"{prefix}/git/commits/{develop['sha']}")
    require(develop_commit.get("sha") == develop["sha"] and type(develop_commit.get("tree")) is dict, "typed develop commit required")
    require(tree_blob(workflow["tree"], PACKAGE_WORKFLOW, fetch) == tree_blob(oid(develop_commit["tree"].get("sha")), PACKAGE_WORKFLOW, fetch),
            "package workflow differs from develop")
    return run


def package_artifact(approval, run, prefix, fetch):
    require(re.fullmatch(r"release-(?:proof-)?(?:mac|x86_64-linux|aarch64-linux|seed)", prefix), "package artifact prefix refused")
    pattern = re.compile(rf"{prefix}-{approval['source']['commit']}-([1-9][0-9]*)\Z")
    matches = []
    for page in range(1, 11):
        response = fetch(f"/repos/{REPOSITORY}/actions/runs/{run['id']}/artifacts?per_page=100&page={page}")
        artifacts = response.get("artifacts")
        require(type(artifacts) is list and len(artifacts) <= 100, "package artifact list refused")
        for artifact in artifacts:
            match = pattern.fullmatch(str(artifact.get("name")))
            if match and int(match[1]) <= run["run_attempt"] and artifact.get("expired") is False:
                matches.append({**artifact, "attempt": int(match[1])})
        if len(artifacts) < 100:
            break
    else:
        raise ValueError("package artifact inventory exceeds its bound")
    require(matches, "live artifact for an accepted package attempt required")
    latest = max(artifact["attempt"] for artifact in matches)
    matches = [artifact for artifact in matches if artifact["attempt"] == latest]
    require(len(matches) == 1, "one artifact for the exact package attempt required")
    artifact = matches[0]
    require(type(artifact.get("id")) is int and artifact["id"] > 0
            and type(artifact.get("digest")) is str and artifact["digest"].startswith("sha256:"), "package artifact identity required")
    checked_hash(artifact["digest"][7:])
    origin = artifact.get("workflow_run", {})
    require(origin.get("id") == run["id"] and origin.get("head_sha") == approval["workflow"]["commit"]
            and origin.get("head_branch") == run["head_branch"], "artifact producer differs")
    return {field: artifact[field] for field in ("id", "name", "digest", "attempt")}


def attestation_result(approval, run, receipt_path, bundle_path, scratch):
    workflow = approval["workflow"]
    identity = f"https://github.com/{REPOSITORY}/{PACKAGE_WORKFLOW}@{workflow['ref']}"
    # gh makes cert-identity and signer-workflow mutually exclusive. The exact
    # SAN owns the workflow path and ref; immutable signer claims are checked
    # below as well. Only the pinned verifier runs; candidate bytes remain data.
    try:
        result = subprocess.run([approval["gh"]["path"], "attestation", "verify", str(receipt_path),
                                 "--bundle", str(bundle_path), "--repo", REPOSITORY,
                                 "--cert-identity", identity, "--cert-oidc-issuer", "https://token.actions.githubusercontent.com",
                                 "--signer-digest", workflow["commit"], "--source-digest", workflow["commit"],
                                 "--source-ref", workflow["ref"], "--deny-self-hosted-runners", "--format", "json"],
                                env={"PATH": "/usr/bin:/bin", "HOME": str(scratch), "GH_CONFIG_DIR": str(scratch / "gh"),
                                     "GH_HOST": "github.com", "GH_PROMPT_DISABLED": "1", "GH_NO_UPDATE_NOTIFIER": "1"},
                                capture_output=True, timeout=120)
    except subprocess.TimeoutExpired as error:
        raise unavailable("attestation verification timed out") from error
    if result.returncode != 0 and UNAVAILABLE_OUTPUT.search(result.stderr):
        raise unavailable("attestation verification could not reach its trust services")
    require(result.returncode == 0 and len(result.stdout) <= 8 * MAX_JSON, "approved build attestation verification failed")
    return json.loads(result.stdout, object_pairs_hook=pairs,
                      parse_constant=lambda _: (_ for _ in ()).throw(ValueError("JSON constant refused")))


def verify_attested_receipt(approval, run, receipt_data, bundle_data, scratch):
    with tempfile.TemporaryDirectory(prefix="provenance-", dir=scratch) as directory:
        directory = Path(directory)
        receipt_path, bundle_path = directory / "receipt.json", directory / "bundle.jsonl"
        receipt_path.write_bytes(receipt_data)
        bundle_path.write_bytes(bundle_data)
        verified = attestation_result(approval, run, receipt_path, bundle_path, directory)
    require(type(verified) is list and verified, "verified build attestation required")
    workflow = approval["workflow"]
    uri = f"https://github.com/{REPOSITORY}/{PACKAGE_WORKFLOW}@{workflow['ref']}"
    invocation = f"https://github.com/{REPOSITORY}/actions/runs/{run['id']}/attempts/{run['run_attempt']}"
    expected = {"issuer": "https://token.actions.githubusercontent.com", "subjectAlternativeName": uri,
                "buildSignerURI": uri, "buildSignerDigest": workflow["commit"], "runnerEnvironment": "github-hosted",
                "sourceRepositoryURI": f"https://github.com/{REPOSITORY}", "sourceRepositoryDigest": workflow["commit"],
                "sourceRepositoryIdentifier": str(run["repository"]["id"]),
                "sourceRepositoryRef": workflow["ref"], "buildConfigURI": uri, "buildConfigDigest": workflow["commit"],
                "buildTrigger": "workflow_dispatch", "runInvocationURI": invocation}
    for entry in verified:
        require(type(entry) is dict and type(entry.get("verificationResult")) is dict, "verified attestation result refused")
        result = entry.get("verificationResult", {})
        require(type(result.get("signature")) is dict and type(result["signature"].get("certificate")) is dict
                and type(result.get("statement")) is dict, "verified certificate/statement required")
        certificate = result.get("signature", {}).get("certificate", {})
        statement = result.get("statement", {})
        if (all(certificate.get(key) == value for key, value in expected.items())
                and result.get("verifiedTimestamps") and statement.get("predicateType") == "https://slsa.dev/provenance/v1"
                and type(statement.get("subject")) is list
                and any(type(subject) is dict and type(subject.get("digest")) is dict
                        and subject["digest"].get("sha256") == sha256(receipt_data) for subject in statement["subject"])):
            return parse_json(receipt_data)
    raise ValueError("attestation certificate does not bind the approved producer, run and attempt")


def verify_release_provenance(policy, manifest, records, root_fd, fetch, scratch, input_root):
    pin = policy.get("approval")
    require(type(pin) is dict and set(pin) == {"path", "sha256"}, "independent approval pin required")
    approval, digest = approved_release(pin["path"], input_root=input_root, expected_hash=pin["sha256"])
    require(approval["source"] == {field: policy[field] for field in ("commit", "tree")}, "source differs from independent approval")
    require(manifest["installer"]["stamps"] == approval["delivery"]
            and policy["publisher_did"] == approval["delivery"]["MAINTAINER_DID"], "delivery differs from independent approval")
    provenance = manifest.get("provenance")
    require(type(provenance) is dict and set(provenance) == {"schema", "run_id", "run_attempt", "approval_sha256", "receipts"}
            and provenance["schema"] == PROVENANCE_SCHEMA and provenance["approval_sha256"] == digest, "approved package provenance required")
    run = checked_package_run(approval, provenance["run_id"], fetch)
    require(type(provenance["run_attempt"]) is int and provenance["run_attempt"] == run["run_attempt"], "package attempt differs")
    receipts = provenance["receipts"]
    platforms = set(manifest["release"]["platforms"])
    require(type(receipts) is list and len(receipts) == len(platforms), "each release platform needs producer proof")
    covered, seen, platform_components, templates = {}, set(), {}, set()
    for entry in receipts:
        require(type(entry) is dict and set(entry) == {"platform", "receipt", "bundle", "components", "artifact", "proof_artifact"},
                "platform provenance refused")
        platform = entry["platform"]
        require(platform in platforms and platform not in seen, "duplicate or foreign platform provenance")
        seen.add(platform)
        artifact = package_artifact(approval, run, "release-mac" if platform == "aarch64-darwin" else "release-" + platform, fetch)
        require(entry["artifact"] == artifact, "platform artifact differs from GitHub")
        proof = package_artifact(approval, run, "release-proof-mac" if platform == "aarch64-darwin" else "release-proof-" + platform, fetch)
        require(entry["proof_artifact"] == proof and artifact["attempt"] <= proof["attempt"], "platform proof artifact differs")
        receipt_data = regular_bytes(relative_path(entry["receipt"]), MAX_JSON, root_fd=root_fd)
        bundle_data = regular_bytes(relative_path(entry["bundle"]), 8 * MAX_JSON, root_fd=root_fd)
        receipt = verify_attested_receipt(approval, {**run, "run_attempt": proof["attempt"]}, receipt_data, bundle_data, scratch)
        require(receipt.get("producer") == {"artifact": artifact, "run_id": run["id"], "workflow_commit": approval["workflow"]["commit"],
                                            "attestation_attempt": proof["attempt"]}, "attested build artifact differs")
        require(receipt.get("schema") == "elastos.release-platform-input/v1" and receipt.get("platform") == platform
                and receipt.get("version") == policy["version"] and receipt.get("source", {}).get("clean") is True
                and {field: receipt.get("source", {}).get(field) for field in ("commit", "tree")} == approval["source"],
                "attested platform source/version differs")
        require(type(receipt.get("files")) is dict, "attested artifact inventory required")
        for name, record in receipt["files"].items():
            if name.startswith("artifacts/"):
                name = relative_path(name[len("artifacts/"):])
                value = (checked_hash(record.get("sha256")), record.get("size"))
                require(type(value[1]) is int and value[1] > 0 and (name not in covered or covered[name] == value), "attested artifact record differs")
                covered[name] = value
        require(f"elastos-{platform}" in covered, "attested native Runtime required")
        components_data = regular_bytes(relative_path(entry["components"]), MAX_JSON, root_fd=root_fd)
        require(attested_file(receipt, "components.json") == (sha256(components_data), len(components_data)),
                "platform components differ from attested receipt")
        platform_components[platform] = parse_json(components_data)
        templates.add(attested_file(receipt, "components-template.json"))
    # Generated components are re-derived from the approved source template and
    # the attested platform manifests; every other file is attested bytes.
    template = git_blob(tree_blob(approval["source"]["tree"], "components.json", fetch), fetch)
    require(templates == {(sha256(template), len(template))}, "attested components template differs from approved source")
    derived = json_bytes(bind_component_cids(merge_components(parse_json(template), platform_components),
                                             {name: record["cid"] for name, record in records.items()}))
    generated = {f"components-{platform}.json" for platform in platforms}
    for name in generated:
        require(name in records, "release components required: " + name)
        data = regular_bytes(relative_path(name), MAX_JSON, root_fd=root_fd)
        require(sha256(data) == records[name]["sha256"] and json_bytes(parse_json(data)) == derived,
                "release components differ from attested inputs: " + name)
    for name, record in records.items():
        require(name in generated or covered.get(name) == (record["sha256"], record["size"]), "publication bytes lack approved producer proof: " + name)
    for platform, descriptor in manifest["release"]["platforms"].items():
        name = f"elastos-{platform}"
        require(name in records and descriptor["binary"]["sha256"] == records[name]["sha256"], "release Runtime differs from attested platform")
    return approval


def attested_file(receipt, name):
    record = receipt["files"].get(name)
    require(type(record) is dict and type(record.get("size")) is int, "attested receipt lacks " + name)
    return checked_hash(record.get("sha256")), record["size"]


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
    capsule_records: tuple


@dataclass(frozen=True)
class PreparedCommunityProfile:
    publisher_did: str
    profile: bytes
    grant: bytes
    network_id: str
    signer_dids: tuple


def check_community_bootstrap_peer(peer):
    require(type(peer) is dict and set(peer) == {"node_id", "connect_ticket"}, "Community bootstrap fields refused")
    node, encoded = peer["node_id"], peer["connect_ticket"]
    require(type(node) is str and HASH.fullmatch(node), "canonical Community node ID required")
    require(type(encoded) is str and 0 < len(encoded) <= 8 * 1024
            and re.fullmatch(r"[a-z2-7]+", encoded), "bounded canonical Community ticket required")
    ticket_bytes = base64.b32decode(encoded.upper() + "=" * ((-len(encoded)) % 8))
    require(base64.b32encode(ticket_bytes).decode().lower().rstrip("=") == encoded,
            "Community ticket base32 differs")
    ticket = parse_json(ticket_bytes)
    require(set(ticket) == {"endpoints", "topic"} and ticket["topic"] is None
            and type(ticket["endpoints"]) is list and 0 < len(ticket["endpoints"]) <= 8,
            "Community ticket endpoints refused")
    for endpoint in ticket["endpoints"]:
        require(type(endpoint) is dict and set(endpoint) == {"id", "addrs"}
                and endpoint["id"] == node and type(endpoint["addrs"]) is list,
                "Community ticket node binding differs")
    require(json_bytes(ticket) == ticket_bytes, "Community ticket JSON is not canonical")
    # Transport-address semantics are checked by the key-free Runtime verifier
    # before this exact approved output is admitted into release preparation.


def prepare_community_profile(policy, root, manifest_name, fetch):
    verify_source(policy, fetch)
    require(policy.get("operation") == "community-profile", "Community policy operation required")
    check_did(policy.get("publisher_did"))
    approval = policy.get("community_profile")
    require(type(approval) is dict and set(approval) == {
        "expected_network_id", "conversation_id", "revision", "previous_profile_sha256",
        "trusted_profile_signer_dids", "profile_payload_sha256", "default_conversation_grant_sha256",
    }, "Community approval fields refused")
    network = approval["expected_network_id"]
    conversation = approval["conversation_id"]
    require(type(network) is str and re.fullmatch(r"[a-z0-9][a-z0-9._:-]{0,127}", network), "Community network ID refused")
    require(type(conversation) is str and re.fullmatch(r"[a-z0-9._:-]{1,128}", conversation), "Community conversation ID refused")
    require(type(approval["revision"]) is int and approval["revision"] == 1
            and approval["previous_profile_sha256"] is None, "only initial Community revision is supported")
    signers = approval["trusted_profile_signer_dids"]
    require(type(signers) is list and 0 < len(signers) <= 32
            and all(type(signer) is str for signer in signers)
            and len(set(signers)) == len(signers) and policy["publisher_did"] in signers,
            "Community signer set refused")
    for signer in signers:
        check_did(signer)
    for quota in ("max_file_bytes", "max_snapshot_bytes"):
        require(type(policy.get(quota)) is int and 0 < policy[quota] < 2**63, "trusted snapshot quota required")
    held_root = directory_fd(root)
    try:
        data = regular_bytes(relative_path(manifest_name), MAX_JSON, root_fd=held_root)
    finally:
        os.close(held_root)
    require(len(data) <= policy["max_file_bytes"] and sha256(data) == checked_hash(policy.get("manifest_sha256")),
            "Community input differs from operator approval")
    manifest = parse_json(data)
    require(set(manifest) == {"schema", "source", "domain", "profile", "default_conversation_grant", "trusted_profile_signer_dids"}
            and manifest["schema"] == COMMUNITY_INPUT_SCHEMA and manifest["domain"] == COMMUNITY_PROFILE_DOMAIN,
            "Community input schema/domain refused")
    require(manifest["source"] == {field: policy[field] for field in ("commit", "tree")}
            and manifest["trusted_profile_signer_dids"] == signers, "Community source/signer set differs")
    profile = manifest["profile"]
    require(type(profile) is dict and set(profile) == {
        "schema", "network_id", "revision", "signer_did", "bootstrap_peers", "default_conversation",
    }, "Community initial profile fields refused")
    require(profile["schema"] == COMMUNITY_PROFILE_SCHEMA and profile["network_id"] == network
            and type(profile["revision"]) is int and profile["revision"] == 1
            and profile["signer_did"] == policy["publisher_did"], "Community profile identity differs")
    peers = profile["bootstrap_peers"]
    # Release preparation verifies the file with the Runtime's operator receipt, which names exactly
    # one bootstrap peer; a profile it cannot verify must not be signed. More peers come later (0.8.1).
    require(type(peers) is list and len(peers) == 1,
            "a Community profile needs exactly one bootstrap peer (release preparation verifies one)")
    for peer in peers:
        check_community_bootstrap_peer(peer)
    require(len({peer["node_id"] for peer in peers}) == len(peers)
            and len({peer["connect_ticket"] for peer in peers}) == len(peers), "duplicate Community bootstrap route")
    grant = manifest["default_conversation_grant"]
    require(type(grant) is dict and set(grant) == {"schema", "network_id", "conversation_id", "sender_service", "admission_policy"}
            and grant == {"schema": COMMUNITY_GRANT_SCHEMA, "network_id": network, "conversation_id": conversation,
                          "sender_service": "chat", "admission_policy": "profile_scoped_signer"},
            "Community default conversation grant differs")
    profile_bytes, grant_bytes = json_bytes(profile), json_bytes(grant)
    require(len(grant_bytes) <= MAX_COMMUNITY_GRANT_BYTES
            and sha256(grant_bytes) == checked_hash(approval["default_conversation_grant_sha256"]), "Community grant hash differs")
    require(profile["default_conversation"] == {"grant_cid": raw_cid(grant_bytes)}, "Community grant CID differs")
    require(sha256(profile_bytes) == checked_hash(approval["profile_payload_sha256"]), "Community payload hash differs")
    envelope = json_bytes({"payload": profile, "signature": "0" * 128, "signer_did": policy["publisher_did"]})
    require(len(envelope) <= MAX_COMMUNITY_PROFILE_BYTES, "Community profile bytes exceed their bound")
    prepared = PreparedCommunityProfile(policy["publisher_did"], profile_bytes, grant_bytes, network, tuple(signers))
    publication = community_profile_publication(prepared, envelope)
    require(all(len(data) <= policy["max_file_bytes"] for _, data in publication)
            and sum(len(data) for _, data in publication) <= policy["max_snapshot_bytes"], "Community snapshot exceeds quota")
    return prepared


def community_profile_summary(prepared):
    """What the custodian signs, shown before the DID is typed."""
    profile, grant = parse_json(prepared.profile), parse_json(prepared.grant)
    lines = ["approved initial Community profile",
             f"  network {profile['network_id']}, conversation {grant['conversation_id']}, revision {profile['revision']}",
             f"  trusted profile signers {', '.join(prepared.signer_dids)}"]
    for peer in profile["bootstrap_peers"]:
        encoded = peer["connect_ticket"]
        ticket = parse_json(base64.b32decode(encoded.upper() + "=" * ((-len(encoded)) % 8)))
        addrs = [json.dumps(addr, sort_keys=True, separators=(",", ":")) for endpoint in ticket["endpoints"] for addr in endpoint["addrs"]]
        lines.append(f"  bootstrap node {peer['node_id']} at {', '.join(addrs) or 'no direct address'}")
    lines += [f"  profile payload sha256 {sha256(prepared.profile)}", f"  grant sha256 {sha256(prepared.grant)}\n"]
    return "\n".join(lines)


def community_profile_digest(payload):
    return hashlib.sha256(COMMUNITY_PROFILE_DOMAIN.encode() + b"\0" + payload).digest()


def verify_community_profile_envelope(prepared, data, backend):
    require(type(data) is bytes and 0 < len(data) <= MAX_COMMUNITY_PROFILE_BYTES, "Community envelope byte bound refused")
    envelope = parse_json(data)
    require(set(envelope) == {"payload", "signature", "signer_did"} and json_bytes(envelope) == data,
            "Community envelope is not canonical")
    require(json_bytes(envelope["payload"]) == prepared.profile
            and envelope["signer_did"] == prepared.publisher_did
            and public_did(backend.public_key()) == prepared.publisher_did, "Community envelope differs from approved signer/payload")
    signature = envelope["signature"]
    require(type(signature) is str and re.fullmatch(r"[0-9a-f]{128}", signature), "Community Ed25519 signature refused")
    require(backend.verify(community_profile_digest(prepared.profile), bytes.fromhex(signature)) is True,
            "Community signature public verification failed")


def sign_community_profile(prepared, backend):
    require(public_did(backend.public_key()) == prepared.publisher_did, "custodian public DID differs")
    signature = backend.sign(community_profile_digest(prepared.profile))
    require(type(signature) is bytes and len(signature) == 64, "Ed25519 signature length differs")
    envelope = json_bytes({"payload": parse_json(prepared.profile), "signature": signature.hex(), "signer_did": prepared.publisher_did})
    verify_community_profile_envelope(prepared, envelope, backend)
    return community_profile_publication(prepared, envelope)


def community_profile_publication(prepared, envelope):
    config = json_bytes({"schema": COMMUNITY_STARTUP_SCHEMA, "expected_network_id": prepared.network_id,
                         "trusted_profile_signer_dids": list(prepared.signer_dids),
                         "profile_chain_base64": [base64.b64encode(envelope).decode()],
                         "default_conversation_grant_base64": base64.b64encode(prepared.grant).decode()})
    pin = json_bytes({"collaboration_network": {"head_cid": raw_cid(config), "expected_network_id": prepared.network_id,
                                             "trusted_profile_signer_dids": list(prepared.signer_dids)}})
    return (("collaboration-network-release-v1.json", config), ("collaboration-network-release-pin-v1.json", pin))


def check_collaboration_network_pin(pin, data):
    """The Runtime's release-pin checks (verify_release_network_bytes) before any key use:
    the pin names these exact bytes, their network and their complete signer set."""
    require(type(pin) is dict and set(pin) == {"head_cid", "expected_network_id", "trusted_profile_signer_dids"},
            "Community network pin fields refused")
    require(pin["head_cid"] == raw_cid(data), "Community network head differs from its file")
    config = parse_json(data)
    require(type(config) is dict and config.get("schema") == COMMUNITY_STARTUP_SCHEMA,
            "Community network file is not a startup configuration")
    require(type(pin["expected_network_id"]) is str and config.get("expected_network_id") == pin["expected_network_id"],
            "Community network ID differs from the release pin")
    signers = (pin["trusted_profile_signer_dids"], config.get("trusted_profile_signer_dids"))
    require(all(type(dids) is list and dids and all(type(did) is str for did in dids) and len(set(dids)) == len(dids)
                for dids in signers) and sorted(signers[0]) == sorted(signers[1]),
            "Community signer set differs from the release pin")


def prepare(policy, root, manifest_name, fetch, snapshot_root):
    held_root = directory_fd(root)
    try:
        return prepare_from_root(policy, root, manifest_name, fetch, snapshot_root, held_root)
    finally:
        os.close(held_root)


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
    require(set(manifest) == {"source", "version", "channel", "files", "installer", "release", "head", "provenance"}, "signing input fields refused")
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
    verify_release_provenance(policy, manifest, records, held_root, fetch, snapshot_root, root)
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
    capsule_records = {}
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
            for payload in capsule_record_payloads(component, policy["publisher_did"], release["version"]):
                key = (payload["name"], payload["release_path"])
                require(capsule_records.setdefault(key, payload) == payload,
                        f"platform components disagree on capsule record: {key[0]} ({key[1]})")
            for field, name, subject in PINNED_RELEASE_FILES:
                pin = component.get(field)
                if pin is None:
                    continue
                require(type(pin) is dict and type(pin.get("head_cid")) is str, f"{subject} pin refused")
                record = records.get(name)
                require(type(record) is dict, f"{subject} snapshot required")
                data = regular_bytes(snapshot_root / name, MAX_JSON)
                require(pin["head_cid"] == raw_cid(data), f"{subject} head differs from snapshot")
                if field == "collaboration_network":
                    check_collaboration_network_pin(pin, data)
                admit_ref(record["cid"], record["sha256"], record["size"], name)
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
    capsule_records = tuple(capsule_records[key] for key in sorted(capsule_records))
    return Prepared(policy["publisher_did"], tuple(snapshot), json_bytes(release), head["updated_at"], head["prev_head_cid"],
                    capsule_records)


def signature_digest(domain, payload):
    require(domain in ("elastos.release.v1", "elastos.release.head.v1", MODEL_CATALOG_DOMAIN, CAPSULE_RECORD_DOMAIN),
            "signing domain refused")
    return hashlib.sha256(domain.encode() + b"\0" + payload).digest()


def envelope_bytes(domain, payload, publisher_did, sign):
    signature = sign(domain, payload)
    require(type(signature) is bytes and len(signature) == 64, "Ed25519 signature length differs")
    return json_bytes({"payload": payload, "signature": signature.hex(), "signer_did": publisher_did})


def backend_signer(backend):
    def sign(domain, payload):
        digest = signature_digest(domain, json_bytes(payload))
        signature = backend.sign(digest)
        require(type(signature) is bytes and len(signature) == 64, "Ed25519 signature length differs")
        require(backend.verify(digest, signature) is True, "signature public verification failed")
        return signature
    return sign


def signed_envelope(domain, payload, publisher_did, backend):
    return envelope_bytes(domain, payload, publisher_did, backend_signer(backend))


def signed_release_bytes(release, capsule_records, publisher_did, sign):
    """The release envelope; each capsule record is signed on its own, then listed
    at the release top level under `capsule_records`."""
    records = [parse_json(envelope_bytes(CAPSULE_RECORD_DOMAIN, payload, publisher_did, sign)) for payload in capsule_records]
    return envelope_bytes("elastos.release.v1", {**release, "capsule_records": records}, publisher_did, sign)


def sign_publication(prepared, backend):
    # Placeholder signatures have the signed length, so this bound is exact before any key use.
    placeholder = signed_release_bytes(parse_json(prepared.release), prepared.capsule_records, prepared.publisher_did,
                                       lambda domain, payload: bytes(64))
    require(len(placeholder) <= MAX_RELEASE_METADATA, "single-chunk release metadata required")
    require(public_did(backend.public_key()) == prepared.publisher_did, "custodian public DID differs")
    release = signed_release_bytes(parse_json(prepared.release), prepared.capsule_records, prepared.publisher_did,
                                   backend_signer(backend))
    head = {"schema": "elastos.release.head/v1", "channel": parse_json(prepared.release)["channel"],
            "version": parse_json(prepared.release)["version"], "latest_release_cid": unixfs_metadata_cid(release),
            "release_sha256": sha256(release), "updated_at": prepared.updated_at,
            "signer_did": prepared.publisher_did, "prev_head_cid": prepared.prev_head_cid}
    return prepared.files + (("release.json", release), ("release-head.json", signed_envelope("elastos.release.head.v1", head, prepared.publisher_did, backend)))


def model_path(value):
    # elastos_common::validate_model_content_path
    require(type(value) is str and len(value) <= 256
            and re.fullmatch(r"(?:[A-Za-z0-9._-]*[A-Za-z0-9_-]/)*[A-Za-z0-9._-]*[A-Za-z0-9_-]", value),
            "model_content path must be a bounded canonical relative path")
    return value


def model_capsule(capsule):
    """Mirror CapsuleManifest::validate for model content. Only passive fields are
    admitted; execution, authority and presentation fields are refused."""
    require(type(capsule) is dict and {"schema", "version", "name", "role", "type", "entrypoint", "model_content"} <= set(capsule)
            <= {"schema", "version", "name", "description", "author", "role", "type", "projections", "entrypoint",
                "model_content", "requires", "capabilities", "interfaces"}, "model capsule manifest fields refused")
    require(capsule["schema"] == "elastos.capsule/v1", "model capsule schema refused")
    require(capsule["role"] == "content" and capsule["type"] == "data" and capsule.get("projections", []) in ([], ["content"])
            and all(capsule.get(field, []) == [] for field in ("requires", "capabilities", "interfaces")),
            "model_content requires passive content/data without execution authority")
    control = re.compile(r"[\x00-\x1f\x7f-\x9f]")
    name = capsule["name"]
    require(type(name) is str and 0 < len(name.encode()) <= 128 and not control.search(name)
            and not any(char.isspace() for char in name), "model_content name refused")
    for field, limit in (("version", 32), ("description", 1024), ("author", 128)):
        value = capsule.get(field)
        require((value is None and field != "version") or (type(value) is str and len(value.encode()) <= limit and not control.search(value)),
                "model_content display facts exceed their bounds")
    require(capsule["version"].strip() != "", "manifest version must not be empty")
    entrypoint = model_path(capsule["entrypoint"])
    require(entrypoint.endswith(".gguf") and ".." not in entrypoint, "model_content entrypoint must name its GGUF file")
    model = capsule["model_content"]
    require(type(model) is dict and set(model) == {"format", "quantization", "engine", "consumer_interface",
                                                   "consumer_interface_version", "minimum_memory_mb", "license", "provenance"},
            "model_content fields refused")
    require(model["format"] == "gguf" and model["quantization"] in ("Q1_0", "Q4_K_M", "Q8_0") and model["engine"] == "llama.cpp"
            and model["consumer_interface"] == "elastos.provider.model" and model["consumer_interface_version"] == "0.1.0"
            and type(model["minimum_memory_mb"]) is int and 1 <= model["minimum_memory_mb"] <= 1_048_576,
            "unsupported model_content format, engine, interface or resource requirement")
    provenance = model["provenance"]
    require(type(provenance) is dict and set(provenance) == {"base_repository", "base_revision", "base_license",
                                                             "quantized_repository", "quantized_revision", "path"},
            "model_content provenance fields refused")
    for license in (model["license"], provenance["base_license"]):
        require(type(license) is dict and set(license) == {"spdx_id", "path"} and license["spdx_id"] == "Apache-2.0",
                "model_content first profile requires Apache-2.0 license facts")
        model_path(license["path"])
    model_path(provenance["path"])
    for repository in (provenance["base_repository"], provenance["quantized_repository"]):
        require(model_path(repository).count("/") == 1, "model_content provenance requires an owner/repository identifier")
    for revision in (provenance["base_revision"], provenance["quantized_revision"]):
        require(type(revision) is str and re.fullmatch(r"[0-9a-f]{40}", revision), "model_content provenance requires an exact lowercase Git revision")


def model_closure(capsule, closure):
    """Mirror the Runtime's self-contained model closure checks (content.rs)."""
    capsule_bytes = json_bytes(capsule)
    require(len(capsule_bytes) <= 64 * 1024, "model capsule manifest exceeds its byte limit")
    require(type(closure) is dict and set(closure) <= {"schema", "kind", "content_digest", "files", "links", "object_did", "publisher_did"}
            and closure.get("schema") == "elastos.content.object.manifest/v1" and closure.get("kind") == "capsule"
            and closure.get("links", []) == [] and closure.get("object_did") is None, "model catalog requires a self-contained capsule closure")
    files = closure.get("files")
    require(type(files) is list and 1 <= len(files) <= 32, "model content closure file count exceeds its limit")
    digest, total, previous, folded = hashlib.sha256(), 0, "", set()
    for item in files:
        require(type(item) is dict and set(item) == {"path", "sha256", "size"}, "invalid model content file fields")
        require(model_path(item["path"]) > previous and item["path"].lower() not in folded and item["path"].lower() != "_elastos_object.json"
                and type(item["size"]) is int and 0 < item["size"] <= 16 * 1024**3
                and (item["path"] == capsule["entrypoint"] or item["size"] <= 1024**2)
                and type(item["sha256"]) is str and HASH.fullmatch(item["sha256"]), "invalid, aliased or unbounded model content file")
        previous = item["path"]
        folded.add(previous.lower())
        total += item["size"]
        digest.update(f"{item['path']}\0{item['sha256']}\0{item['size']}\0".encode())
    require(total <= 16 * 1024**3 and closure.get("content_digest") == "sha256:" + digest.hexdigest(), "model closure size or digest mismatch")
    model = capsule["model_content"]
    notices = (model["license"]["path"], model["provenance"]["base_license"]["path"], model["provenance"]["path"])
    paths = {item["path"]: item for item in files}
    require(all(path in paths for path in (capsule["entrypoint"], *notices)), "model content reference is absent from the closure")
    require(all(path not in (capsule["entrypoint"], "capsule.json") for path in notices), "model notice must be a distinct closure file")
    require(paths.get("capsule.json") == {"path": "capsule.json", "sha256": sha256(capsule_bytes), "size": len(capsule_bytes)},
            "model closure capsule.json differs from capsule_manifest")


def check_model_catalog(payload, publisher_did, now):
    """Refuse a payload the Runtime's verify_model_catalog would refuse, before any key use."""
    require(type(payload) is dict and {"schema", "published_at", "entries"} <= set(payload) <= {"schema", "published_at", "expires_at", "entries"},
            "model catalog payload fields refused")
    require(payload["schema"] == MODEL_CATALOG_SCHEMA, "model catalog schema refused")
    published, expires = payload["published_at"], payload.get("expires_at")
    require(type(published) is int and 0 <= published <= now, "model catalog published_at must be a past Unix time")
    require(expires is None or (type(expires) is int and expires > now and expires > published and expires < 2**64), "model catalog expires_at must follow now")
    entries = payload["entries"]
    require(type(entries) is list and 1 <= len(entries) <= 8, "model catalog requires 1 to 8 entries")
    cids, names = set(), set()
    for entry in entries:
        require(type(entry) is dict and set(entry) == {"cid", "capsule_manifest", "object_manifest"}, "model catalog entry fields refused")
        require(type(entry["cid"]) is str and entry["cid"].startswith("b") and cid_info(entry["cid"])[0] == 0x70
                and entry["cid"] not in cids, "model package requires a unique canonical DAG-PB SHA-256 CIDv1")
        cids.add(entry["cid"])
        capsule, closure = entry["capsule_manifest"], entry["object_manifest"]
        model_capsule(capsule)
        require(capsule["name"] not in names, "model catalog entries require unique capsule names")
        names.add(capsule["name"])
        require(type(closure) is not dict or closure.get("publisher_did") in (None, publisher_did), "model closure publisher must match the catalog signer")
        model_closure(capsule, closure)
    # The hex signature has a fixed length, so the signed size is known before signing.
    require(len(json_bytes({"payload": payload, "signature": "0" * 128, "signer_did": publisher_did})) <= MAX_MODEL_CATALOG,
            "signed model catalog exceeds 128 KiB")


def sign_model_catalog(payload, publisher_did, backend, now):
    check_model_catalog(payload, publisher_did, now)
    require(public_did(backend.public_key()) == publisher_did, "custodian public DID differs")
    return signed_envelope(MODEL_CATALOG_DOMAIN, payload, publisher_did, backend)


def model_catalog_summary(payload):
    return "".join(f"  {entry['capsule_manifest']['name']}  {entry['cid']}\n" for entry in payload["entries"])


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


def confirmed(publisher_did, subject, input_stream, output_stream):
    output_stream.write(f"Sign {subject} as {publisher_did}.\nType this complete DID to confirm, or press Enter to cancel: ")
    output_stream.flush()
    return input_stream.readline().strip() == publisher_did


def write_outputs(output_root, outputs):
    output_root.mkdir(mode=0o700)
    try:
        for name, data in outputs:
            destination = output_root / name
            destination.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
            if isinstance(data, Path):
                data.rename(destination)
            else:
                with destination.open("xb") as output:
                    output.write(data)
            destination.chmod(0o444)
    except Exception:
        shutil.rmtree(output_root)
        raise


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--policy", required=True, type=Path)
    parser.add_argument("--input-root", required=True, type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--manifest", default="signing-input.json")
    mode.add_argument("--model-catalog", metavar="PAYLOAD", help="sign this model catalogue payload instead of a release")
    parser.add_argument("--output-root", required=True, type=Path)
    parser.add_argument("--operation", choices=("release", "community-profile"), default="release")
    args = parser.parse_args()
    require(args.model_catalog is None or args.operation == "release",
            "model catalogue and Community profile operations cannot be combined")
    require(sys.flags.isolated == 1 and sys.flags.no_site == 1, "run the pinned interpreter with -I -S")
    require(args.input_root.is_absolute() and args.policy.is_absolute() and args.output_root.is_absolute(), "absolute input/policy/output paths required")
    require(args.input_root == args.input_root.resolve() and args.policy == args.policy.resolve()
            and args.output_root == args.output_root.resolve(), "canonical custodian/input paths required")
    require(not args.policy.is_relative_to(args.input_root) and not args.output_root.is_relative_to(args.input_root), "custodian paths must be outside input root")
    policy = parse_json(regular_bytes(args.policy, MAX_JSON, trusted=True))
    require(policy.get("operation", "release") == args.operation, "policy operation differs from command")
    pinned_tools(policy, args.input_root)
    require(not args.output_root.exists(), "output root already exists")
    for path in (args.output_root.parent, *args.output_root.parent.parents):
        info = path.stat()
        require(info.st_uid in (0, os.getuid()) and not info.st_mode & 0o022,
                "output parent protection refused")
    with tempfile.TemporaryDirectory(prefix=".elastos-signing-", dir=args.output_root.parent) as scratch:
        if args.model_catalog is not None:
            did = policy["publisher_did"]
            check_did(did)
            data = regular_bytes(args.input_root / relative_path(args.model_catalog), MAX_JSON)
            require(sha256(data) == checked_hash(policy.get("model_catalog_sha256")), "model catalogue payload differs from policy approval")
            payload = parse_json(data)
            check_model_catalog(payload, did, int(time.time()))
            subject = f"approved model catalogue payload sha256:{sha256(data)} with entries\n{model_catalog_summary(payload)}"
            require(confirmed(did, subject, sys.stdin, sys.stderr), "signing cancelled")
            backend = OpenSSLBackend(policy, args.input_root, Path(scratch))
            try:
                catalog = sign_model_catalog(payload, did, backend, int(time.time()))
            finally:
                backend.close()
            write_outputs(args.output_root, (("model-catalog.json", catalog),))
            print(f"Signed model catalogue. Pin model_catalog.head_cid: {raw_cid(catalog)}")
            return
        if args.operation == "community-profile":
            prepared = prepare_community_profile(policy, args.input_root, args.manifest, github_json)
            require(confirmed(prepared.publisher_did, community_profile_summary(prepared), sys.stdin, sys.stderr),
                    "signing cancelled")
        else:
            prepared = prepare(policy, args.input_root, args.manifest, github_json, Path(scratch))
            approval, digest = approved_release(policy["approval"]["path"], args.input_root, policy["approval"]["sha256"])
            delivery = approval["delivery"]
            print(f"Approved source: {approval['source']['commit']} tree {approval['source']['tree']}\n"
                  f"Approved builder: {approval['workflow']['commit']} {approval['workflow']['ref']} tree {approval['workflow']['tree']}\n"
                  f"Approved delivery: {delivery['PUBLISHER_GATEWAY']} node {delivery['PUBLISHER_NODE_ID']} IPNS {delivery['IPNS_NAME']}\n"
                  f"Independent approval SHA-256: {digest}", file=sys.stderr)
            require(confirmed(prepared.publisher_did, "approved release", sys.stdin, sys.stderr), "signing cancelled")
        # Recheck canonical source authority after confirmation, before backend use.
        verify_source(policy, github_json)
        if args.operation == "release":
            approved_release(policy["approval"]["path"], args.input_root, policy["approval"]["sha256"])
        backend = OpenSSLBackend(policy, args.input_root, Path(scratch))
        try:
            publication = (sign_community_profile(prepared, backend) if args.operation == "community-profile"
                           else sign_publication(prepared, backend))
        finally:
            backend.close()
        write_outputs(args.output_root, publication)
    print("Signed approved Community profile snapshot." if args.operation == "community-profile"
          else "Signed approved publication snapshot.")


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.SubprocessError) as error:
        sys.exit(f"Release signing refused: {error}")
