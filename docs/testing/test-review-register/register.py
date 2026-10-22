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


def records(manifest):
    for part in manifest["partitions"]:
        yield from read_lines(part["file"])


def git(repo, *arguments):
    return subprocess.check_output(["git", "-C", str(repo), *arguments], timeout=30)


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
    if errors:
        raise SystemExit("Verification failed:\n" + "\n".join(errors))
    print("Verified {} file entries, {} typed records and {} finding groups{}.".format(
        len(files), len(seen), len(findings), " against pinned Git objects" if repo else " against artifact receipts"))


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
    args = parser.parse_args()
    manifest = read_json("manifest.json")
    if args.command == "verify":
        verify(manifest, args.git_repo)
        return
    if args.command == "summary":
        print(json.dumps({key: manifest[key] for key in ("source", "artifact_kind", "counts", "partitions", "count_rules")}, indent=2))
        return
    count = 0
    profiles = read_json("profiles.json") if args.json else {}
    for row in records(manifest):
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
