#!/usr/bin/env python3
"""Read and verify this test-review snapshot with the Python standard library."""

import argparse
import collections
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parent


def read_json(name):
    return json.loads((ROOT / name).read_text(encoding="utf-8"))


def read_lines(name):
    with (ROOT / name).open(encoding="utf-8") as stream:
        for line in stream:
            if line.strip():
                yield json.loads(line)


def records(manifest, followup=False):
    if followup:
        for entry in manifest.get("source_followups", []):
            yield from read_json(entry["file"])["reviews"]
        return
    for part in manifest["partitions"]:
        yield from read_lines(part["file"])


def git(repo, *arguments):
    return subprocess.check_output(["git", "-C", str(repo), *arguments], timeout=30)


def verify_followups(manifest, repo, errors):
    total = 0
    for entry in manifest.get("source_followups", []):
        supplement = read_json(entry["file"])
        source = supplement["source"]
        pin, tree = source["candidate"], source["tree"]
        if source != entry["source"] or pin == manifest["source"]["candidate"]:
            errors.append("follow-up source separation: " + entry["file"])
        rows = supplement["reviews"]
        ids = {row["id"] for row in rows}
        files = {row["path"]: row for row in supplement["files"]}
        findings = {row["id"]: row for row in supplement["findings"]}
        if len(ids) != len(rows) or len(files) != len(supplement["files"]) or len(findings) != len(supplement["findings"]):
            errors.append("duplicate follow-up identity: " + entry["file"])
        for row in rows:
            origin, review = row["source"], row["review"]
            path = origin["path"]
            if origin["commit"] != pin or origin["tree"] != tree or path not in files or origin["file_sha256"] != files[path]["sha256"]:
                errors.append("follow-up row source: " + row["id"])
            if not 1 <= origin["line"] <= origin["end_line"] <= files[path]["lines"] or len(origin["body_sha256"]) != 64:
                errors.append("follow-up source range/hash: " + row["id"])
            if review["body_read"] is not True or not review["reason"] or not review["actions"] or not review["scope"]:
                errors.append("follow-up review evidence: " + row["id"])
            if set(row["finding_ids"]) - set(findings):
                errors.append("follow-up unresolved finding: " + row["id"])
        for identity, finding in findings.items():
            expected = {row["id"] for row in rows if identity in row["finding_ids"]}
            if set(finding["affected_ids"]) != expected:
                errors.append("follow-up finding links: " + identity)
        counts = supplement["counts"]
        declarations = {(row["source"]["path"], row["source"]["line"], row["source"]["end_line"]) for row in rows}
        if counts != entry["counts"] or counts["expanded_registrations"] != len(rows) or counts["declarations"] != len(declarations) or counts["test_files"] != len({row["source"]["path"] for row in rows}) or counts["changed_files"] != len(files) or counts["dispositions"] != dict(collections.Counter(row["review"]["disposition"] for row in rows)):
            errors.append("follow-up counts: " + entry["file"])
        receipt_ref = supplement["execution_receipt"]
        receipt_raw = (ROOT / receipt_ref["file"]).read_bytes()
        if hashlib.sha256(receipt_raw).hexdigest() != receipt_ref["sha256"]:
            errors.append("follow-up execution receipt hash")
        receipt = json.loads(receipt_raw)
        if receipt["source_commit"] != pin or receipt["source_tree"] != tree:
            errors.append("follow-up execution source")
        run_names = {run["name"] for run in receipt["runs"]}
        for row in rows:
            if row["execution"]["receipt"] != receipt_ref["file"] or row["execution"]["run"] not in run_names:
                errors.append("follow-up execution link: " + row["id"])
        if repo:
            if git(repo, "rev-parse", pin + "^{tree}").decode().strip() != tree:
                errors.append("follow-up Git tree")
            hashes = {path: value["sha256"] for path, value in files.items()}
            for path, digest in receipt["files"].items():
                if path in hashes and hashes[path] != digest:
                    errors.append("follow-up source/receipt disagreement: " + path)
                hashes[path] = digest
            for path, digest in hashes.items():
                if hashlib.sha256(git(repo, "show", pin + ":" + path)).hexdigest() != digest:
                    errors.append("follow-up pinned source hash: " + path)
        total += len(rows)
    return total


def verify(manifest, repo):
    errors = []
    profiles = read_json("profiles.json")
    for field, values in profiles.items():
        for key, value in values.items():
            raw = json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")).encode()
            if hashlib.sha256(raw).hexdigest() != key:
                errors.append("profile content identity: " + field + ":" + key)
    for name, receipt in manifest["artifacts"].items():
        path = ROOT / name
        if not path.is_file():
            errors.append("missing artifact: " + name)
            continue
        raw = path.read_bytes()
        if len(raw) != receipt["bytes"] or hashlib.sha256(raw).hexdigest() != receipt["sha256"]:
            errors.append("artifact hash/size: " + name)
    file_rows = list(read_lines("files.jsonl"))
    files = {row["path"]: row for row in file_rows}
    findings = read_json("findings.json")
    finding_ids = {row["id"] for row in findings}
    if len(files) != len(file_rows) or len(finding_ids) != len(findings):
        errors.append("duplicate file or finding identity")
    seen = set()
    byfile = collections.Counter()
    body_reads = 0
    for part in manifest["partitions"]:
        rows = list(read_lines(part["file"]))
        if len(rows) != part["rows"] or sum(r["review"]["body_read"] for r in rows) != part["body_read_rows"]:
            errors.append("partition counts: " + part["name"])
        if dict(collections.Counter(r["kind"] for r in rows)) != part["kinds"]:
            errors.append("partition kinds: " + part["name"])
        for row in rows:
            identity, source, review = row["id"], row["source"], row["review"]
            if identity in seen:
                errors.append("duplicate record: " + identity)
            seen.add(identity)
            path = source["path"]
            byfile[path] += 1
            body_reads += review["body_read"]
            if path not in files or files[path]["file_sha256"] != source["file_sha256"]:
                errors.append("source-file identity: " + identity)
            if source["commit"] != manifest["source"]["candidate"] or source["tree"] != manifest["source"]["tree"]:
                errors.append("candidate identity: " + identity)
            if not 1 <= source["line"] <= source["end_line"]:
                errors.append("source range: " + identity)
            if len(source["body_sha256"]) != 64:
                errors.append("body hash: " + identity)
            if not review["body_read"] and review["disposition_status"] != "provisional":
                errors.append("missing provisional label: " + identity)
            if not review["reason"] or not review["actions"]:
                errors.append("missing review reason/action: " + identity)
            if set(row["finding_ids"]) - finding_ids:
                errors.append("unresolved finding: " + identity)
            for field in profiles:
                if row[field].get("profile") not in profiles[field]:
                    errors.append("unresolved profile: " + identity + ":" + field)
    for finding in findings:
        if set(finding["affected_ids"]) - seen:
            errors.append("unresolved affected record: " + finding["id"])
    for path, row in files.items():
        if byfile[path] != row["register_rows"]:
            errors.append("file record count: " + path)
    counts = manifest["counts"]
    if (len(files), len(seen), body_reads, len(seen) - body_reads, len(findings)) != (
        counts["tracked_files"], counts["register_rows"], counts["body_read_rows"],
        counts["provisional_body_rows"], counts["findings"]
    ):
        errors.append("manifest totals")
    if repo:
        commit = manifest["source"]["candidate"]
        if git(repo, "rev-parse", commit + "^{tree}").decode().strip() != manifest["source"]["tree"]:
            errors.append("Git source tree")
        tracked = set(git(repo, "ls-tree", "-r", "--name-only", commit).decode().splitlines())
        if tracked != set(files):
            errors.append("Git tracked-file census")
        for path, row in files.items():
            if hashlib.sha256(git(repo, "show", commit + ":" + path)).hexdigest() != row["file_sha256"]:
                errors.append("pinned source hash: " + path)
    followup_rows = verify_followups(manifest, repo, errors)
    if errors:
        raise SystemExit("Verification failed:\n" + "\n".join(errors))
    print("Verified {} file entries, {} typed records and {} finding groups{}.".format(
        len(files), len(seen), len(findings), " against pinned Git objects" if repo else " against artifact receipts"))
    if followup_rows:
        print("Verified {} separately pinned follow-up records; main totals remain separate.".format(followup_rows))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=("summary", "find", "verify"))
    parser.add_argument("--path", default="", help="source-path substring")
    parser.add_argument("--name", default="", help="case-identity substring")
    parser.add_argument("--finding", help="exact finding identity")
    parser.add_argument("--disposition", help="proposed disposition, such as improve")
    parser.add_argument("--provisional", action="store_true", help="show bodies awaiting direct review")
    parser.add_argument("--json", action="store_true", help="emit complete matching JSON records")
    parser.add_argument("--git-repo", type=Path, help="repository containing the pinned commit, for source verification")
    parser.add_argument("--followup", action="store_true", help="search separately pinned follow-up rows instead of the main census")
    args = parser.parse_args()
    manifest = read_json("manifest.json")
    if args.command == "verify":
        verify(manifest, args.git_repo)
        return
    if args.command == "summary":
        result = {key: manifest[key] for key in ("source", "artifact_kind", "counts", "partitions", "count_rules")}
        result["source_followups"] = manifest.get("source_followups", [])
        print(json.dumps(result, indent=2))
        return
    count = 0
    profiles = read_json("profiles.json") if args.json and not args.followup else {}
    for row in records(manifest, args.followup):
        source, review = row["source"], row["review"]
        if args.path not in source["path"] or args.name.lower() not in row["id"].lower():
            continue
        if args.finding and args.finding not in row["finding_ids"]:
            continue
        if args.disposition and args.disposition != review["disposition"]:
            continue
        if args.provisional and review["body_read"]:
            continue
        count += 1
        if args.json:
            for field, values in profiles.items():
                row[field] = values[row[field]["profile"]]
            print(json.dumps(row, ensure_ascii=False))
        else:
            print("{}:{} [{}; {}]\n  {}\n  {}".format(
                source["path"], source["line"], review["disposition"], review["method"], row["id"],
                " ".join(review["actions"])))
    if not args.json:
        print("{} matching records.".format(count))


if __name__ == "__main__":
    main()
