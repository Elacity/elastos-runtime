#!/usr/bin/env python3
"""Prepare V4 signing data from verified V3 support and the original CI N4 Runtime.

This workflow tool creates a publication assembly, not a new native build receipt.
It reads current public V3 metadata and the operator's actual committed receipt.
Runtime CID evidence comes from the isolated builder's qualified add_path import.
The seed rechecks that multi-block UnixFS CID when it imports the signed set.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tempfile
import time
import urllib.request

sys.dont_write_bytecode = True

COMMIT = "1d39373898e6bd286071336e0c829d45e14cb831"
TREE = "174349f043b10db0b186f64da851c7a5b16eca6f"
DID = "did:key:z6MkgwHd2BCWe1jHMXPiR6H1q1RFPcv1YzhMbK5G1kBarbfe"
PLATFORM = "aarch64-darwin"
RUNTIME = "elastos-" + PLATFORM
ORIGIN = "https://elastos.elacitylabs.com"
LIMIT = 2 * 1024**2


def require(condition, message):
    if not condition:
        raise ValueError(message)


def protected(path, directory=True):
    path = Path(path)
    require(path.is_absolute() and path == path.resolve(strict=True), "canonical absolute path required")
    info = path.lstat()
    require(info.st_uid == os.geteuid() and info.st_mode & 0o077 == 0,
            "private owned path required")
    require(stat.S_ISDIR(info.st_mode) if directory else
            stat.S_ISREG(info.st_mode) and info.st_nlink == 1, "regular owned path required")
    for parent in path.parents:
        info = parent.lstat()
        require(stat.S_ISDIR(info.st_mode) and info.st_uid in (0, os.geteuid())
                and info.st_mode & 0o022 == 0, "protected path ancestry required")
    return path


def read(path, limit=LIMIT):
    protected(path, False)
    with Path(path).open("rb") as stream:
        data = stream.read(limit + 1)
    require(0 < len(data) <= limit, "bounded metadata required")
    return data


def write(path, data):
    descriptor = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "wb") as stream:
        stream.write(data)


def reserve(parent, needed, measure=shutil.disk_usage):
    usage = measure(parent)
    require((usage.free - needed) * 100 >= usage.total * 15, "copy requires 15% free space")


def check_checkouts(source, workflow_commit):
    source = protected(source)
    workflow = Path(__file__).resolve().parents[2]
    for root, expected, tree in ((source, COMMIT, TREE), (workflow, workflow_commit, None)):
        require(re.fullmatch(r"[0-9a-f]{40}", expected) is not None, "exact checkout commit required")
        def git(*args):
            return subprocess.check_output(["git", *args], cwd=root, text=True).strip()
        require(git("rev-parse", "HEAD") == expected, "checkout commit differs")
        require(tree is None or git("rev-parse", "HEAD^{tree}") == tree, "source tree differs")
        require(not git("status", "--porcelain=v1", "--untracked-files=all"), "clean checkout required")
    return workflow


def source_modules(source, workflow_commit):
    workflow = check_checkouts(source, workflow_commit)
    spec = importlib.util.spec_from_file_location("canary_v4_native", source / "scripts/release-platform-input.py")
    native = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(native)
    signer, _ = native.model_tools()
    return native, signer, workflow


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def fetch_public(name):
    require(name in ("release.json", "release-head.json", "install.sh"), "fixed public path required")
    request = urllib.request.Request(ORIGIN + "/" + name, headers={"User-Agent": "Elastos-V4-finalizer"})
    with urllib.request.build_opener(NoRedirect()).open(request, timeout=30) as reply:
        require(reply.status == 200, "public V3 read failed")
        data = reply.read(LIMIT + 1)
    require(0 < len(data) <= LIMIT, "public metadata exceeds bound")
    return data


def verify_envelope(data, domain, signer, openssl, scratch):
    envelope = signer.parse_json(data)
    require(set(envelope) == {"payload", "signature", "signer_did"}
            and envelope["signer_did"] == DID and type(envelope["payload"]) is dict
            and type(envelope["signature"]) is str
            and re.fullmatch(r"[0-9a-f]{128}", envelope["signature"]) is not None,
            "public signature envelope differs")
    number = 0
    for char in DID[len("did:key:z"):]:
        number = number * 58 + signer.BASE58.index(char)
    with tempfile.TemporaryDirectory(prefix="signature-", dir=scratch) as temporary:
        root = Path(temporary)
        write(root / "public.der", bytes.fromhex("302a300506032b6570032100") + number.to_bytes(34, "big")[2:])
        write(root / "digest", signer.signature_digest(domain, signer.json_bytes(envelope["payload"])))
        write(root / "signature", bytes.fromhex(envelope["signature"]))
        reply = subprocess.run([str(openssl), "pkeyutl", "-provider", "default", "-verify", "-rawin", "-pubin",
                        "-keyform", "DER", "-inkey", "public.der", "-in", "digest", "-sigfile", "signature"],
                       cwd=root, env={"OPENSSL_CONF": "/dev/null", "LANG": "C"},
                       stdin=subprocess.DEVNULL, capture_output=True, check=True, timeout=20)
        require(len(reply.stdout) <= 65536 and len(reply.stderr) <= 65536, "public verifier output exceeds bound")
    return envelope["payload"]


def admit(args, native, signer, scratch, fetch=fetch_public, verify_signature=verify_envelope):
    for root in (args.n3, args.n4, args.finalized_n3, args.unsigned_v3):
        protected(root)
        for path in root.rglob("*"):
            protected(path, path.is_dir())
    n3, n4, final = (native.verify(root) for root in (args.n3, args.n4, args.finalized_n3))
    for receipt, version in ((n3, "0.8.0-alpha.3"), (n4, "0.8.0-alpha.4"), (final, "0.8.0-alpha.3")):
        require(receipt["source"]["commit"] == COMMIT and receipt["source"]["tree"] == TREE
                and receipt["platform"] == PLATFORM and receipt["version"] == version,
                "native source/platform/version differs")
    require("support_origin" not in n3 and "model_finalization" not in n3
            and "support_origin" in n4 and "model_finalization" not in n4
            and "support_origin" not in final and "model_finalization" in final,
            "original and finalized native origins differ")
    require(args.n3.parent == args.n4.parent, "original native pair requires one admitted artifact root")
    pair = signer.parse_json(read(args.n3.parent / "receipts/native-pair.json"))
    require(pair["schema"] == "elastos.canary-union-native-pair/v1"
            and pair["source_commit"] == COMMIT and pair["source_tree"] == TREE
            and str(pair["source_ci_run"]) == "37238542483"
            and pair["source_clean_before_and_after"] is True
            and pair["support_origin_matches_V3"] is True and pair["models_exported"] is True
            and pair["publisher_did"] == DID and re.fullmatch(r"[0-9a-f]{40}", pair["workflow_commit"])
            and str(pair["run"]).isdigit() and int(pair["run"]) > 0
            and str(pair["attempt"]).isdigit() and int(pair["attempt"]) > 0,
            "original CI native pair admission differs")
    require(type(pair["native"]) is list and len(pair["native"]) == 2
            and {record["input"] for record in pair["native"]} == {"N3", "N4"},
            "original CI native pair inventory differs")
    for record in pair["native"]:
        receipt = n3 if record["input"] == "N3" else n4
        require(record["version"] == receipt["version"]
                and record["stdout"] == "elastos " + receipt["version"] + "\n" and record["stderr"] == ""
                and record["binary_sha256"] == receipt["files"]["artifacts/" + RUNTIME]["sha256"],
                "original CI Runtime evidence differs")
    original = read(args.n3 / "platform-input.json")
    require(read(args.n4 / "support-input.json") == original
            and read(args.finalized_n3 / "model-native-input.json") == original,
            "original N3 support receipt differs")
    proof = final["model_finalization"]
    require(proof["publisher_did"] == DID and proof["native_receipt_sha256"] == signer.sha256(original),
            "finalized N3 provenance differs")
    runtime_pin = n4["files"]["artifacts/" + RUNTIME]
    imported = signer.parse_json(read(args.runtime_import))
    require(set(imported) == {"sha256", "size", "cid"} and
            {key: imported[key] for key in ("sha256", "size")} ==
            {key: runtime_pin[key] for key in ("sha256", "size")}, "Runtime import receipt differs")
    require(signer.cid_info(imported["cid"])[0] == 0x70, "Runtime requires an imported UnixFS CID")
    catalogue = read(args.finalized_n3 / "artifacts/model-catalog.json", 128 * 1024)
    envelope, _, tool = native.public_catalog_verification(catalogue, DID, args.openssl, scratch)
    components = signer.parse_json(read(args.finalized_n3 / "components.json"))
    require(components["model_catalog"]["head_cid"] == signer.raw_cid(catalogue)
            and components["model_catalog"]["publisher_dids"] == [DID]
            and envelope["signer_did"] == DID, "signed catalogue binding differs")
    manifest = signer.parse_json(read(args.unsigned_v3 / "signing-input.json"))
    require(manifest["source"] == {"commit": COMMIT, "tree": TREE}
            and manifest["version"] == "0.8.0-alpha.3" and manifest["channel"] == "canary"
            and set(manifest["release"]["platforms"]) == {PLATFORM}, "V3 signing identity differs")
    require(manifest["release"]["platforms"][PLATFORM] == {
        "binary": manifest["files"][RUNTIME],
        "components": manifest["files"]["components-" + PLATFORM + ".json"]},
        "public V3 platform descriptors differ")
    require({path.relative_to(args.unsigned_v3).as_posix() for path in args.unsigned_v3.rglob("*") if path.is_file()}
            == set(manifest["files"]) | {"signing-input.json"}, "V3 signing inventory differs")
    artifacts = {name.removeprefix("artifacts/"): pin for name, pin in final["files"].items()
                 if name.startswith("artifacts/")}
    require(set(manifest["files"]) == set(artifacts) | {"components-" + PLATFORM + ".json"},
            "V3 finalized artifact inventory differs")
    for name, pin in manifest["files"].items():
        signer.relative_path(name)
        actual = native.file_record(args.unsigned_v3 / name)
        require({key: actual[key] for key in ("sha256", "size")} ==
                {key: pin[key] for key in ("sha256", "size")}, "V3 signing artifact differs")
        signer.cid_info(pin["cid"])
        if name in artifacts:
            require(actual["sha256"] == artifacts[name]["sha256"] and actual["size"] == artifacts[name]["size"],
                    "V3 signing artifact differs from finalized N3")
    public = {name: fetch(name) for name in ("release.json", "release-head.json", "install.sh")}
    release = verify_signature(public["release.json"], "elastos.release.v1", signer, args.openssl, scratch)
    head = verify_signature(public["release-head.json"], "elastos.release.head.v1", signer, args.openssl, scratch)
    release_cid = signer.unixfs_metadata_cid(public["release.json"])
    head_cid = signer.unixfs_metadata_cid(public["release-head.json"])
    require(release == manifest["release"], "public V3 differs from admitted signing input")
    require(head == {"schema": "elastos.release.head/v1", "channel": "canary", "version": "0.8.0-alpha.3",
            "latest_release_cid": release_cid, "release_sha256": signer.sha256(public["release.json"]),
            "updated_at": manifest["head"]["updated_at"], "signer_did": DID,
            "prev_head_cid": manifest["head"]["prev_head_cid"]}, "public V3 head differs")
    _, template = native.installer_source_blob(manifest["source"])
    require(public["install.sh"] == signer.render_installer(template, manifest["installer"]["stamps"], DID)
            and signer.sha256(public["install.sh"]) == release["installer_sha256"], "public installer differs")
    state = signer.parse_json(read(args.publish_state, 65536))
    require(set(state) == {"publisher_did", "last_release_cid", "last_head_cid", "last_version", "last_published_at"}
            and state["publisher_did"] == DID and state["last_release_cid"] == release_cid
            and state["last_head_cid"] == head_cid and state["last_version"] == "0.8.0-alpha.3"
            and type(state["last_published_at"]) is int
            and head["updated_at"] <= state["last_published_at"] <= int(time.time()) + 300,
            "actual committed V3 receipt differs")
    return n4, final, manifest, public, state, imported, tool, pair


def finalize(args, native, signer, workflow, fetch=fetch_public, verify_signature=verify_envelope,
             measure=shutil.disk_usage):
    output = args.output
    protected(output.parent)
    require(output.is_absolute() and output == output.parent / output.name
            and not output.exists() and not output.is_symlink(), "fresh output required")
    for root in (args.n3, args.n4, args.finalized_n3, args.unsigned_v3):
        require(not output.is_relative_to(root) and not root.is_relative_to(output), "output must be outside inputs")
    reserve(output.parent, 8 * LIMIT, measure)
    with tempfile.TemporaryDirectory(prefix=".v4-finalize-", dir=output.parent) as temporary:
        scratch = Path(temporary)
        controls = [root / "platform-input.json" for root in (args.n3, args.n4, args.finalized_n3)]
        controls += [args.unsigned_v3 / "signing-input.json", args.publish_state, args.runtime_import,
                     args.n3.parent / "receipts/native-pair.json"]
        initial = {path: read(path) for path in controls}
        helper_sha = native.digest(Path(__file__))
        n4, final, v3, public, state, imported, tool, pair = admit(args, native, signer, scratch, fetch, verify_signature)
        total = sum(pin["size"] for pin in final["files"].values()) + imported["size"]
        reserve(output.parent, 2 * total + 8 * LIMIT, measure)
        stage = scratch / "stage"
        native.stage_inputs([PLATFORM + "=" + str(args.finalized_n3)], "0.8.0-alpha.3", stage, PLATFORM)
        assembly = json.loads((stage / "assembly.json").read_bytes())
        runtime = stage / "artifacts" / RUNTIME
        runtime.unlink()
        shutil.copyfile(args.n4 / "artifacts" / RUNTIME, runtime)
        runtime.chmod(0o700)
        require(native.file_record(runtime) == n4["files"]["artifacts/" + RUNTIME], "Runtime changed during copy")
        assembly["version"] = "0.8.0-alpha.4"
        assembly["files"][RUNTIME] = n4["files"]["artifacts/" + RUNTIME]
        (stage / "assembly.json").write_bytes(signer.json_bytes(assembly))
        native.verify_staged_inputs(stage, preview_platform=PLATFORM)
        cids = {name: pin["cid"] for name, pin in v3["files"].items()}
        cids[RUNTIME] = imported["cid"]
        native_map, full_map, stamps = (scratch / name for name in ("native-cids.json", "cids.json", "stamps.json"))
        write(native_map, signer.json_bytes({name: cids[name] for name in assembly["files"]}))
        write(full_map, signer.json_bytes(cids))
        write(stamps, signer.json_bytes(v3["installer"]["stamps"]))
        native.attach_input_cids(stage, native_map, PLATFORM)
        require((stage / "artifacts" / ("components-" + PLATFORM + ".json")).read_bytes() ==
                (args.unsigned_v3 / ("components-" + PLATFORM + ".json")).read_bytes(), "V4 support manifest changed")
        result = scratch / "result"
        result.mkdir(mode=0o700)
        unsigned = result / "unsigned-V4"
        native.signing_input(stage, full_map, stamps, "canary", unsigned, PLATFORM,
                             state["last_release_cid"], state["last_head_cid"])
        for root in (args.n3, args.n4, args.finalized_n3):
            native.verify(root)
        require(native.file_record(args.n4 / "artifacts" / RUNTIME) == assembly["files"][RUNTIME],
                "original Runtime changed after copy")
        require(all(read(path) == data for path, data in initial.items()), "input receipt changed during finalization")
        require(all(fetch(name) == data for name, data in public.items()), "public V3 changed during finalization")
        require(native.digest(args.openssl) == tool["sha256"], "OpenSSL changed during finalization")
        check_checkouts(args.source_root, args.workflow_commit)
        require(native.digest(Path(__file__)) == helper_sha, "workflow helper changed during finalization")
        proof = {"schema": "elastos.canary-v4-finalization/v1", "source_commit": COMMIT, "source_tree": TREE,
                 "workflow_commit": args.workflow_commit, "helper_sha256": helper_sha,
                 "native_pair_sha256": native.digest(args.n3.parent / "receipts/native-pair.json"),
                 "native_pair": pair,
                 "native_receipts": {name: native.digest(root / "platform-input.json")
                                     for name, root in (("N3", args.n3), ("N4", args.n4), ("N3-finalized", args.finalized_n3))},
                 "runtime": imported, "public_origin": ORIGIN, "public_signatures_verified": True,
                 "public_metadata_sha256": {name: signer.sha256(data) for name, data in public.items()},
                 "committed_v3_receipt_sha256": native.digest(args.publish_state),
                 "committed_v3_receipt": state, "openssl": tool,
                 "manifest_sha256": native.digest(unsigned / "signing-input.json"),
                 "scope": "Metadata assembly; original native receipts and CI Runtime bytes preserved. Operator owns policy approval and signing."}
        write(result / "V4-finalization.json", signer.json_bytes(proof))
        write(result / "v3-publish-state.json", read(args.publish_state, 65536))
        require(not output.exists() and not output.is_symlink(), "output appeared during finalization")
        result.rename(output)
    return proof


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("source-root", "n3", "n4", "finalized-n3", "unsigned-v3", "publish-state", "runtime-import", "openssl", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--workflow-commit", required=True)
    args = parser.parse_args()
    require(sys.flags.isolated and sys.flags.no_site, "run with Python -I -S")
    native, signer, workflow = source_modules(args.source_root, args.workflow_commit)
    finalize(args, native, signer, workflow)
    print("Prepared V4 metadata; operator policy approval and signing follow.")


if __name__ == "__main__":
    main()
