#!/usr/bin/env python3
"""Hand a signed model content package from one Runtime-managed Kubo to another.

`export` streams the package closure out of the sender's Kubo as a CAR with a
receipt. `verify-car` and `import` replay that closure into the receiver's Kubo
and confirm the recursive retention pin. `pin-catalog` installs the sender's
signed permanent catalog snapshot and points `components.json.model_catalog`
at it without verifying the signature; the Runtime verifies the catalog
signature when it reads the snapshot, so the receipt reports that as pending.
The retention pin and the catalog trust pin are separate and stay separate.
Stdlib only. Every command prints one JSON receipt on success and one
`error: ...` line on refusal.
"""

import argparse
import base64
import contextlib
import dataclasses
import hashlib
import http.client
import json
import os
import re
import shutil
import stat
import sys
import tempfile
import time
import urllib.parse

KUBO_VERSION = "0.40.1"
CHUNK_BYTES = 1024 * 1024
MAX_CONTROL_BYTES = 64 * 1024
MAX_CATALOG_BYTES = 128 * 1024
MAX_COMPONENTS_BYTES = 4 * 1024 * 1024
MAX_BUDGET = 2**63 - 1
CATALOG_SCHEMA = "elastos.model.catalog/v1"
RECEIPT_SCHEMA = "elastos.model.package-car/v1"
IMPORT_SCHEMA = "elastos.model.package-import/v1"
CATALOG_PIN_SCHEMA = "elastos.model.catalog-pin/v1"
BASE32_ALPHABET = frozenset("abcdefghijklmnopqrstuvwxyz234567")

# Kubo 0.40.1 config/import.go treats null as exactly these defaults; the
# Runtime's directory_hash.rs accepts the same spellings and nothing else.
IMPORT_PROFILE = {
    "Import.UnixFSHAMTDirectorySizeThreshold": lambda value: value is None
    or (isinstance(value, int) and not isinstance(value, bool) and value == 262144)
    or value in ("256KiB", "262144"),
    "Import.UnixFSHAMTDirectorySizeEstimation": lambda value: value is None or value == "links",
}


class Refusal(Exception):
    pass


@dataclasses.dataclass(frozen=True)
class CarDigest:
    sha256: str
    size: int


@dataclasses.dataclass(frozen=True)
class ImportStats:
    blocks: int
    block_bytes: int


@dataclasses.dataclass(frozen=True)
class Receipt:
    package_cid: str
    car_sha256: str
    car_bytes: int
    kubo_version: str
    exported_at: int

    def to_json(self):
        return {"schema": RECEIPT_SCHEMA, **dataclasses.asdict(self)}

    @classmethod
    def load(cls, path):
        try:
            record = json.loads(read_bounded(path, MAX_CONTROL_BYTES))
        except ValueError:
            raise Refusal(f"receipt {path} is not JSON")
        fields = {field.name: field.type for field in dataclasses.fields(cls)}
        if (
            not isinstance(record, dict)
            or record.get("schema") != RECEIPT_SCHEMA
            or set(record) != {"schema", *fields}
            or any(not isinstance(record[name], kind) or isinstance(record[name], bool) for name, kind in fields.items())
            or record["car_bytes"] <= 0
            or len(record["car_sha256"]) != 64
        ):
            raise Refusal(f"receipt {path} is not a {RECEIPT_SCHEMA} record")
        return cls(**{name: record[name] for name in fields})


@dataclasses.dataclass(frozen=True)
class CatalogPin:
    head_cid: str
    publisher_dids: tuple
    max_cache_bytes: int
    max_model_memory_bytes: int

    def to_json(self):
        return {
            "head_cid": self.head_cid,
            "publisher_dids": list(self.publisher_dids),
            "local_use": {
                "max_cache_bytes": self.max_cache_bytes,
                "max_model_memory_bytes": self.max_model_memory_bytes,
            },
        }


class TrailerResponse(http.client.HTTPResponse):
    """http.client discards chunked trailers; Kubo reports a mid-stream failure only in X-Stream-Error."""

    def __init__(self, *args, **kwargs):
        super().__init__(*args, **kwargs)
        self.trailers = {}

    def _read_and_discard_trailer(self):
        while True:
            line = self.fp.readline(65537)
            if len(line) > 65536:
                raise http.client.LineTooLong("trailer line")
            if not line or line in (b"\r\n", b"\n"):
                return
            name, _, value = line.decode("latin-1").partition(":")
            self.trailers[name.strip()] = value.strip()


class KuboApi:
    def __init__(self, base_url, timeout):
        match = re.fullmatch(r"http://([^/:]+):([1-9][0-9]{0,4})/?", base_url)
        if not match:
            raise Refusal(f"--kubo-api must look like http://127.0.0.1:PORT, got {base_url!r}")
        self.host, self.port, self.timeout = match.group(1), int(match.group(2)), timeout
        self.base_url = f"http://{self.host}:{self.port}"

    @classmethod
    def from_data_dir(cls, data_dir, timeout):
        coords_path = os.path.join(data_dir, "ipfs-coords.json")
        try:
            coords = json.loads(read_bounded(coords_path, MAX_CONTROL_BYTES))
            port = coords["api_port"]
        except (OSError, ValueError, KeyError, TypeError):
            raise Refusal(f"{coords_path} does not name the Runtime Kubo api_port; is the Runtime's Kubo running?")
        return cls(f"http://127.0.0.1:{port}", timeout)

    def _connect(self):
        connection = http.client.HTTPConnection(self.host, self.port, timeout=self.timeout)
        connection.response_class = TrailerResponse
        return connection

    def _open(self, operation, query=None):
        target = "/api/v0/" + operation + ("?" + urllib.parse.urlencode(query) if query else "")
        connection = self._connect()
        try:
            connection.request("POST", target)
            return connection, connection.getresponse()
        except OSError as error:
            connection.close()
            raise Refusal(f"kubo api {self.base_url} unreachable: {error}")

    def _json(self, operation, query=None):
        connection, response = self._open(operation, query)
        with contextlib.closing(connection):
            body = response.read(MAX_CONTROL_BYTES + 1)
        text = body[:300].decode("utf-8", "replace").strip()
        if response.status != 200 or len(body) > MAX_CONTROL_BYTES:
            raise Refusal(f"kubo {operation} answered {response.status}: {text}")
        try:
            return json.loads(body)
        except ValueError:
            raise Refusal(f"kubo {operation} returned invalid JSON: {text}")

    def verify_profile(self):
        version = self._json("version").get("Version")
        if version != KUBO_VERSION:
            raise Refusal(f"kubo version {version!r} is not {KUBO_VERSION}")
        for key, accepts in IMPORT_PROFILE.items():
            answer = self._json("config", {"arg": key})
            if answer.get("Key") != key or "Value" not in answer or not accepts(answer["Value"]):
                raise Refusal(f"kubo import profile {key}={answer.get('Value')!r} is not canonical")
        return version

    def export_car(self, cid, destination):
        connection, response = self._open("dag/export", {"arg": cid})
        with contextlib.closing(connection):
            if response.status != 200:
                text = response.read(MAX_CONTROL_BYTES)[:300].decode("utf-8", "replace").strip()
                raise Refusal(f"kubo dag/export {cid} answered {response.status}: {text}")
            digest = hashlib.sha256()
            size = 0
            with open(destination, "wb") as sink:
                for chunk in iter(lambda: response.read(CHUNK_BYTES), b""):
                    sink.write(chunk)
                    digest.update(chunk)
                    size += len(chunk)
                sink.flush()
                os.fsync(sink.fileno())
            if response.trailers.get("X-Stream-Error"):
                raise Refusal(f"kubo dag/export {cid} failed mid-stream: {response.trailers['X-Stream-Error']}")
        if size == 0:
            raise Refusal(f"kubo dag/export {cid} returned an empty CAR")
        return CarDigest(digest.hexdigest(), size)

    def repo_path(self):
        path = self._json("repo/stat").get("RepoPath")
        if not isinstance(path, str) or not path:
            raise Refusal("kubo repo/stat did not report RepoPath")
        return path

    def import_car(self, car_path, cid):
        boundary = "elastos-model-package-" + os.urandom(12).hex()
        head = (
            f"--{boundary}\r\n"
            'Content-Disposition: form-data; name="file"; filename="package.car"\r\n'
            "Content-Type: application/vnd.ipld.car\r\n\r\n"
        ).encode("ascii")
        foot = f"\r\n--{boundary}--\r\n".encode("ascii")
        size = os.path.getsize(car_path)
        connection = self._connect()
        try:
            connection.putrequest("POST", "/api/v0/dag/import?pin-roots=true&stats=true")
            connection.putheader("Content-Type", f"multipart/form-data; boundary={boundary}")
            connection.putheader("Content-Length", str(len(head) + size + len(foot)))
            connection.endheaders()
            connection.send(head)
            sent = 0
            with open(car_path, "rb") as source:
                for chunk in iter(lambda: source.read(CHUNK_BYTES), b""):
                    connection.send(chunk)
                    sent += len(chunk)
            if sent != size:
                raise Refusal(f"{car_path} changed size during upload")
            connection.send(foot)
            response = connection.getresponse()
            body = response.read(MAX_CONTROL_BYTES + 1)
        except OSError as error:
            raise Refusal(f"kubo dag/import upload to {self.base_url} failed: {error}")
        finally:
            connection.close()
        return parse_import_events(response.status, body, response.trailers, cid)

    def is_recursively_pinned(self, cid):
        connection, response = self._open("pin/ls", {"arg": cid, "type": "recursive"})
        with contextlib.closing(connection):
            body = response.read(MAX_CONTROL_BYTES + 1)
        if response.status != 200 or len(body) > MAX_CONTROL_BYTES:
            return False
        try:
            keys = json.loads(body).get("Keys", {})
        except (ValueError, AttributeError):
            return False
        return isinstance(keys, dict) and keys.get(cid, {}).get("Type") == "recursive"


def parse_import_events(status, body, trailers, cid):
    text = body.decode("utf-8", "replace")
    if status != 200 or len(body) > MAX_CONTROL_BYTES:
        raise Refusal(f"kubo dag/import answered {status}: {text.strip()[:300]}")
    if trailers.get("X-Stream-Error"):
        raise Refusal(f"kubo dag/import failed mid-stream: {trailers['X-Stream-Error']}")
    root = stats = None
    for line in filter(None, text.splitlines()):
        try:
            event = json.loads(line)
        except ValueError:
            raise Refusal(f"kubo dag/import returned a non-JSON line: {line[:300]}")
        if not isinstance(event, dict) or "Message" in event or "Type" in event:
            raise Refusal(f"kubo dag/import reported an error: {line[:300]}")
        if root is None and isinstance(event.get("Root"), dict):
            root = event["Root"]
        elif stats is None and isinstance(event.get("Stats"), dict):
            stats = event["Stats"]
        else:
            raise Refusal(f"kubo dag/import returned an unexpected event: {line[:300]}")
    if root is None or stats is None:
        raise Refusal("kubo dag/import did not report exactly one root and its stats")
    imported = root.get("Cid", {}).get("/") if isinstance(root.get("Cid"), dict) else None
    if imported != cid:
        raise Refusal(f"kubo dag/import root {imported!r} is not the expected package {cid}")
    if root.get("PinErrorMsg"):
        raise Refusal(f"kubo could not pin {cid}: {root['PinErrorMsg']}")
    try:
        return ImportStats(int(stats["BlockCount"]), int(stats["BlockBytesCount"]))
    except (KeyError, TypeError, ValueError):
        raise Refusal("kubo dag/import stats lack BlockCount and BlockBytesCount")


def raw_cid(data):
    multihash = b"\x12\x20" + hashlib.sha256(data).digest()
    return "b" + base64.b32encode(b"\x01\x55" + multihash).decode("ascii").lower().rstrip("=")


def digest_file(path):
    digest = hashlib.sha256()
    size = 0
    with open(path, "rb") as source:
        for chunk in iter(lambda: source.read(CHUNK_BYTES), b""):
            digest.update(chunk)
            size += len(chunk)
    return CarDigest(digest.hexdigest(), size)


def read_bounded(path, limit):
    with open(path, "rb") as source:
        data = source.read(limit + 1)
    if len(data) > limit:
        raise Refusal(f"{path} exceeds its {limit}-byte bound")
    return data


def check_private_dir(path):
    info = os.lstat(path)
    if not stat.S_ISDIR(info.st_mode):
        raise Refusal(f"{path} must be a real directory, not a symlink or file")
    if info.st_uid != os.geteuid() or info.st_mode & 0o022:
        raise Refusal(f"{path} must be owned by the current user and not writable by group or others")


def write_private(path, data):
    handle, temp_path = tempfile.mkstemp(prefix=".handoff-", dir=os.path.dirname(path) or ".")
    try:
        with os.fdopen(handle, "wb") as sink:
            os.fchmod(handle, 0o600)
            sink.write(data)
            sink.flush()
            os.fsync(sink.fileno())
        os.replace(temp_path, path)
    except BaseException:
        if os.path.exists(temp_path):
            os.unlink(temp_path)
        raise


def check_car(car_path, receipt_path, cid):
    receipt = Receipt.load(receipt_path)
    if receipt.package_cid != cid:
        raise Refusal(f"receipt package_cid {receipt.package_cid} does not match expected CID {cid}")
    size = os.path.getsize(car_path)
    if size != receipt.car_bytes:
        raise Refusal(f"CAR size {size} does not match receipt car_bytes {receipt.car_bytes}")
    digest = digest_file(car_path)
    if digest.sha256 != receipt.car_sha256:
        raise Refusal(f"CAR sha256 {digest.sha256} does not match receipt car_sha256 {receipt.car_sha256}")
    return receipt


def check_catalog(catalog, package_cid, publisher_dids):
    try:
        signed = json.loads(catalog)
    except ValueError:
        raise Refusal("catalog is not JSON")
    if not isinstance(signed, dict) or set(signed) != {"payload", "signature", "signer_did"}:
        raise Refusal("catalog must be a {payload, signature, signer_did} envelope")
    payload = signed["payload"]
    if not isinstance(payload, dict) or payload.get("schema") != CATALOG_SCHEMA:
        raise Refusal(f"catalog payload.schema must be {CATALOG_SCHEMA}")
    if payload.get("expires_at") is not None:
        raise Refusal(
            f"catalog expires_at={payload['expires_at']} is a timed snapshot; this helper pins permanent"
            " snapshots only (sign a payload that omits expires_at)"
        )
    entries = payload.get("entries")
    if not isinstance(entries, list) or not (1 <= len(entries) <= 8):
        raise Refusal("catalog must carry 1 to 8 entries")
    if any(not isinstance(entry, dict) for entry in entries):
        raise Refusal("catalog entries must be objects")
    cids = [entry.get("cid") for entry in entries]
    if len(set(cids)) != len(cids):
        raise Refusal("catalog entries must use unique package CIDs")
    if package_cid not in cids:
        raise Refusal(f"catalog entries do not include --package-cid {package_cid}")
    if signed["signer_did"] not in publisher_dids:
        raise Refusal(f"catalog signer_did {signed['signer_did']!r} is not among the given --publisher-did values")


def package_cid_argument(value):
    if not (value.startswith("bafy") and 8 <= len(value) <= 128 and set(value) <= BASE32_ALPHABET):
        raise argparse.ArgumentTypeError(f"{value!r} is not a base32 CIDv1 starting with bafy")
    return value


def bounded_int(minimum):
    def parse(value):
        number = int(value)
        if not minimum <= number <= MAX_BUDGET:
            raise argparse.ArgumentTypeError(f"{value} is outside {minimum}..{MAX_BUDGET}")
        return number

    return parse


def kubo_from_args(args):
    if args.kubo_api:
        return KuboApi(args.kubo_api, args.timeout_seconds)
    return KuboApi.from_data_dir(args.data_dir, args.timeout_seconds)


def cmd_verify_kubo(args):
    return {"kubo_version": kubo_from_args(args).verify_profile(), "import_profile": "canonical"}


def cmd_head_cid(args):
    catalog = read_bounded(args.catalog, MAX_CATALOG_BYTES)
    return {"head_cid": raw_cid(catalog), "catalog_bytes": len(catalog)}


def cmd_export(args):
    kubo = kubo_from_args(args)
    version = kubo.verify_profile()
    partial = args.output + ".partial"
    try:
        digest = kubo.export_car(args.cid, partial)
    except BaseException:
        if os.path.exists(partial):
            os.unlink(partial)
        raise
    os.replace(partial, args.output)
    receipt = Receipt(args.cid, digest.sha256, digest.size, version, int(time.time()))
    with open(args.output + ".receipt.json", "wb") as sink:
        sink.write(json.dumps(receipt.to_json(), indent=2).encode("utf-8") + b"\n")
    return receipt.to_json()


def cmd_verify_car(args):
    return check_car(args.car, args.receipt, args.cid).to_json()


def cmd_import(args):
    receipt = check_car(args.car, args.receipt, args.cid)
    kubo = kubo_from_args(args)
    version = kubo.verify_profile()
    repo_path = kubo.repo_path()
    try:
        free = shutil.disk_usage(repo_path).free
    except OSError as error:
        raise Refusal(f"cannot measure free space at kubo repo {repo_path}: {error}")
    if free - receipt.car_bytes < args.free_space_floor_bytes:
        raise Refusal(
            f"kubo repo {repo_path} has {free} free bytes; importing {receipt.car_bytes} bytes would"
            f" leave less than the {args.free_space_floor_bytes}-byte --free-space-floor-bytes floor"
        )
    stats = kubo.import_car(args.car, args.cid)
    if not kubo.is_recursively_pinned(args.cid):
        raise Refusal(f"kubo imported {args.cid} but does not list it as a recursive pin")
    return {
        "schema": IMPORT_SCHEMA,
        "package_cid": args.cid,
        "pinned": True,
        "blocks": stats.blocks,
        "block_bytes": stats.block_bytes,
        "kubo_version": version,
    }


def cmd_pin_catalog(args):
    dids = args.publisher_dids
    if len(set(dids)) != len(dids) or len(dids) > 8 or any(not did.startswith("did:key:z") or len(did) > 128 for did in dids):
        raise Refusal("--publisher-did values must be unique did:key:z... identifiers, at most eight")
    check_private_dir(args.data_dir)
    catalog = read_bounded(args.catalog, MAX_CATALOG_BYTES)
    head = raw_cid(catalog)
    if head != args.head_cid:
        raise Refusal(f"catalog head is {head}, not --head-cid {args.head_cid}")
    check_catalog(catalog, args.package_cid, args.publisher_dids)
    pin = CatalogPin(head, tuple(args.publisher_dids), args.max_cache_bytes, args.max_model_memory_bytes)
    components_path = os.path.join(args.data_dir, "components.json")
    try:
        components = json.loads(read_bounded(components_path, MAX_COMPONENTS_BYTES))
    except ValueError:
        raise Refusal(f"{components_path} is not JSON")
    if not isinstance(components, dict):
        raise Refusal(f"{components_path} must be a JSON object")
    components["model_catalog"] = pin.to_json()
    write_private(os.path.join(args.data_dir, "model-catalog.json"), catalog)
    write_private(components_path, json.dumps(components, indent=2).encode("utf-8") + b"\n")
    return {
        "schema": CATALOG_PIN_SCHEMA,
        "head_cid": head,
        "package_cid": args.package_cid,
        **pin.to_json(),
        "signature_verification": "pending_runtime_read",
    }


def add_kubo_arguments(parser):
    target = parser.add_mutually_exclusive_group(required=True)
    target.add_argument("--kubo-api", metavar="URL", help="Kubo HTTP API, e.g. http://127.0.0.1:5001")
    target.add_argument("--data-dir", metavar="DIR", help="Runtime data dir holding ipfs-coords.json")
    parser.add_argument("--timeout-seconds", type=float, default=60.0, help="per-request and per-chunk idle timeout")


def build_parser():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)

    verify_kubo = commands.add_parser("verify-kubo", help="check the pinned Kubo version and canonical import profile")
    add_kubo_arguments(verify_kubo)
    verify_kubo.set_defaults(run=cmd_verify_kubo)

    head_cid = commands.add_parser("head-cid", help="print the raw CIDv1 SHA-256 of a catalog file")
    head_cid.add_argument("--catalog", required=True)
    head_cid.set_defaults(run=cmd_head_cid)

    export = commands.add_parser("export", help="stream a package closure to FILE.car with a receipt")
    export.add_argument("--cid", required=True, type=package_cid_argument)
    export.add_argument("--output", required=True, metavar="FILE.car")
    add_kubo_arguments(export)
    export.set_defaults(run=cmd_export)

    verify_car = commands.add_parser("verify-car", help="check a CAR against its receipt and expected CID")
    verify_car.add_argument("--car", required=True)
    verify_car.add_argument("--receipt", required=True)
    verify_car.add_argument("--cid", required=True, type=package_cid_argument)
    verify_car.set_defaults(run=cmd_verify_car)

    import_ = commands.add_parser("import", help="verify, import and pin a CAR in the receiver's Kubo")
    import_.add_argument("--car", required=True)
    import_.add_argument("--receipt", required=True)
    import_.add_argument("--cid", required=True, type=package_cid_argument)
    import_.add_argument("--free-space-floor-bytes", required=True, type=bounded_int(0))
    add_kubo_arguments(import_)
    import_.set_defaults(run=cmd_import)

    pin_catalog = commands.add_parser("pin-catalog", help="install a permanent catalog snapshot and pin its head")
    pin_catalog.add_argument("--data-dir", required=True)
    pin_catalog.add_argument("--catalog", required=True, metavar="model-catalog.json")
    pin_catalog.add_argument("--head-cid", required=True)
    pin_catalog.add_argument("--publisher-did", dest="publisher_dids", action="append", required=True, metavar="DID")
    pin_catalog.add_argument("--package-cid", required=True, type=package_cid_argument)
    pin_catalog.add_argument("--max-cache-bytes", required=True, type=bounded_int(1))
    pin_catalog.add_argument("--max-model-memory-bytes", required=True, type=bounded_int(1))
    pin_catalog.set_defaults(run=cmd_pin_catalog)
    return parser


def main(argv=None):
    args = build_parser().parse_args(argv)
    try:
        receipt = args.run(args)
    except (Refusal, OSError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    print(json.dumps(receipt))
    return 0


if __name__ == "__main__":
    sys.exit(main())
