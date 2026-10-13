#!/usr/bin/env python3
"""Key-free consumer checks using the installed signer's provenance contract.

RELEASE_APPROVAL is an operator-owned file prepared before downloading inputs.
It approves source H, the complete builder commit/tree W and its canonical ref,
the public delivery tuple, and an installed gh executable by path and SHA-256.
This program never creates that approval. Historical packages without attestations
need a new approved build. No candidate executable runs during these checks.
"""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

spec = importlib.util.spec_from_file_location("release_signer", Path(__file__).with_name("release-signer.py"))
S = importlib.util.module_from_spec(spec)
sys.modules[spec.name] = S
spec.loader.exec_module(S)


def cli_fetch(approval, path):
    S.require(path.startswith(f"/repos/{S.REPOSITORY}/"), "GitHub repository refused")
    try:
        result = subprocess.run([approval["gh"]["path"], "api", "--hostname", "github.com", path],
                                capture_output=True, timeout=30,
                                env={**os.environ, "GH_HOST": "github.com", "GH_PROMPT_DISABLED": "1"})
    except subprocess.TimeoutExpired as error:
        raise S.unavailable("GitHub API timed out") from error
    if result.returncode != 0 and (S.UNAVAILABLE_OUTPUT.search(result.stderr) or not S.HTTP_REFUSAL.search(result.stderr)):
        raise S.unavailable(result.stderr.decode(errors="replace").strip()[:200])
    S.require(result.returncode == 0 and len(result.stdout) <= 8 * S.MAX_JSON, "GitHub package authority refused " + path)
    return S.parse_json(result.stdout)


def check_receipt(approval, run, path, bundle, scratch, fetch, prefix, payload_root, platform=None, version=None, seed=False):
    data = S.regular_bytes(path.absolute(), S.MAX_JSON)
    proof = S.regular_bytes(bundle.absolute(), 8 * S.MAX_JSON)
    artifact = S.package_artifact(approval, run, prefix, fetch)
    proof_artifact = S.package_artifact(approval, run, prefix.replace('release-', 'release-proof-', 1), fetch)
    S.require(artifact['attempt'] <= proof_artifact['attempt'], 'artifact attempt lacks fresh proof')
    receipt = S.verify_attested_receipt(approval, {**run, 'run_attempt': proof_artifact['attempt']}, data, proof, scratch)
    S.require(receipt.get('producer') == {'artifact': artifact, 'run_id': run['id'],
              'workflow_commit': approval['workflow']['commit'], 'attestation_attempt': proof_artifact['attempt']},
              'receipt artifact/run/attempt differs')
    S.require({field: receipt.get("source", {}).get(field) for field in ("commit", "tree")} == approval["source"],
              "attested package source differs from approval")
    if seed:
        S.require(receipt.get("schema") == "elastos.release-seed-input/v1" and type(receipt.get("files")) is dict
                  and set(receipt["files"]) == {"elastos", "ipfs-provider", "source.tar.gz"}, "attested seed receipt required")
        for name, record in receipt["files"].items():
            payload = payload_root / name
            S.require(type(record) is dict and set(record) == {"sha256", "size"}, "seed payload record refused")
            S.require(type(record["size"]) is int and 0 < record["size"] <= 8 * 1024**3, "seed payload size refused")
            digest = S.hashlib.sha256()
            fd = S.file_fd(payload.absolute())
            with os.fdopen(fd, "rb") as source:
                S.require(S.stat.S_ISREG(os.fstat(source.fileno()).st_mode), "regular seed payload required")
                S.require(os.fstat(source.fileno()).st_size == record["size"], "seed payload size differs")
                for chunk in iter(lambda: source.read(1024 * 1024), b""):
                    digest.update(chunk)
            S.require(digest.hexdigest() == S.checked_hash(record["sha256"]), "seed payload differs from attested receipt")
    else:
        S.require(receipt.get("schema") == "elastos.release-platform-input/v1" and receipt.get("platform") == platform
                  and receipt.get("version") == version and receipt.get("source", {}).get("clean") is True,
                  "attested platform/version differs")
    return receipt


def bind(approval_path, approval, digest, work):
    """Add the package proof to the prepared signing input, then freeze it."""
    record = S.parse_json(S.regular_bytes(work / "run.json", S.MAX_JSON))
    unsigned = work / "unsigned"
    manifest_path = unsigned / "signing-input.json"
    manifest = S.parse_json(S.regular_bytes(manifest_path, S.MAX_JSON))
    S.require(record["approval"] == {"path": str(approval_path), "sha256": digest}, "prepared approval differs")
    S.require(manifest["source"] == approval["source"] and manifest["installer"]["stamps"] == approval["delivery"],
              "prepared source/delivery differs from independent approval")
    S.require("provenance" not in manifest, "signing input already bound")
    proof_dir = unsigned / "provenance"
    proof_dir.mkdir(mode=0o700)
    entries = []
    for entry in record["receipts"]:
        platform = entry["platform"]
        S.require(platform in record["platforms"], "prepared platform refused")
        receipt = f"provenance/{platform}.receipt.json"
        bundle = f"provenance/{platform}.bundle.jsonl"
        components = f"provenance/{platform}.components.json"
        platform_input = work / "inputs" / platform / record["input"]
        shutil.copyfile(platform_input / "platform-input.json", unsigned / receipt)
        shutil.copyfile(work / "inputs" / platform / "provenance.jsonl", unsigned / bundle)
        shutil.copyfile(platform_input / "components.json", unsigned / components)
        entries.append({"platform": platform, "receipt": receipt, "bundle": bundle, "components": components,
                        "artifact": entry["artifact"], "proof_artifact": entry["proof_artifact"]})
    manifest["provenance"] = {"schema": S.PROVENANCE_SCHEMA, "run_id": int(record["run"]), "run_attempt": record["run_attempt"],
                              "approval_sha256": digest, "receipts": entries}
    manifest_path.write_bytes(S.json_bytes(manifest))
    for path in [manifest_path, *proof_dir.iterdir()]:
        path.chmod(0o400)


def update_pin(approval, work_root, run, version, pin_path, head_path):
    """Admit only a source whose CI update journey started from the release users run now.

    The source's pin must name the public head. N+1 of a pair keeps the pin of the head before
    N; it passes only if the public head is byte for byte the head this Mac signed for N,
    prepared as N from the same package run and source, and that head follows the pin."""
    pinned = S.parse_json(S.regular_bytes(pin_path, 64 * 1024)).get("head_cid")
    raw = S.regular_bytes(head_path, 256 * 1024)
    public = S.unixfs_metadata_cid(raw)
    if public == pinned:
        return
    try:
        payload = S.parse_json(raw).get("payload")
        other = payload["version"]
        S.require(type(other) is str and S.VERSION.fullmatch(other) and other != version, "not a paired N")
        signed = S.regular_bytes(work_root / other / "signed/release-head.json", 256 * 1024)
        record = S.parse_json(S.regular_bytes(work_root / other / "run.json", S.MAX_JSON))
        source = S.parse_json(S.regular_bytes(work_root / other / "unsigned/signing-input.json", S.MAX_JSON))["source"]
        paired = (signed == raw and record["run"] == str(run) and record["input"] == "N"
                  and source == approval["source"] and payload["prev_head_cid"] == pinned)
    except (ValueError, OSError, KeyError, TypeError):
        paired = False
    S.require(paired, f"the candidate's update journey did not start from the release users run now (pinned head {pinned}, "
                      f"public head {public}); bump the pin with scripts/update-hop-compare.py pin-previous-release and rerun CI")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("approval", "update-pin", "run", "artifact", "receipt", "seed", "bind"))
    parser.add_argument("--approval", required=True, type=Path)
    parser.add_argument("--work-root", required=True, type=Path)
    parser.add_argument("--run", type=int)
    parser.add_argument("--prefix")
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--bundle", type=Path)
    parser.add_argument("--platform")
    parser.add_argument("--payload-root", type=Path)
    parser.add_argument("--version")
    parser.add_argument("--work", type=Path)
    parser.add_argument("--pin", type=Path)
    parser.add_argument("--head", type=Path)
    args = parser.parse_args()
    work_root = args.work_root.resolve()
    approval, digest = S.approved_release(args.approval, input_root=work_root)
    if args.command == "approval":
        print(json.dumps({"sha256": digest, "source": approval["source"], "workflow": approval["workflow"], "delivery": approval["delivery"], "gh": approval["gh"]}))
        return
    if args.command == "update-pin":
        update_pin(approval, work_root, args.run, args.version, args.pin, args.head)
        return
    if args.command == "bind":
        bind(args.approval, approval, digest, args.work.resolve())
        return
    fetch = lambda path: cli_fetch(approval, path)
    run = S.checked_package_run(approval, args.run, fetch)
    if args.command == "run":
        print(run["run_attempt"])
    elif args.command == "artifact":
        artifact = S.package_artifact(approval, run, args.prefix, fetch)
        print(artifact["id"], artifact["digest"][7:], approval["source"]["commit"], artifact["attempt"])
    else:
        work_root.mkdir(mode=0o700, parents=True, exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="verify-provenance-", dir=work_root) as directory:
            receipt = check_receipt(approval, run, args.receipt, args.bundle, Path(directory), fetch, args.prefix, args.payload_root, args.platform, args.version, args.command == "seed")
        print(json.dumps(receipt))


if __name__ == "__main__":
    try:
        main()
    except (ValueError, OSError, subprocess.SubprocessError, KeyError, TypeError) as error:
        sys.exit(f"release-provenance: {error}")
