#!/usr/bin/env python3
"""Refuse raw file deletion added to product code.

Deleting the wrong thing is the failure class review keeps finding: uninstall
through a linked data folder or a case-variant HOME, Browser host retirement
through linked parents and by name prefix, share copy through swapped links,
TURN retirement unlinking through a linked parent. Every deletion added to
product code calls the owned-removal helper of its language, or names on the
same line (or on the comment line directly above) why the raw call cannot
reach anything else:

    # removal-ok: <reason>        // removal-ok: <reason>

Helpers: remove one exact path below an owned root, never through a link,
optionally only when the leaf still has the device+inode the caller recorded.
    Rust    elastos_common::owned_removal::remove_owned(root, relative, expected)
    Python  scripts/lib/owned_removal.py  remove_owned(root, relative, expected)
    shell   python3 scripts/lib/owned_removal.py ROOT RELATIVE [--expect DEV:INO]

Only lines added in BASE...HEAD are checked, so existing code does not flood
the gate. Tests, fixtures, vendored code and the helpers themselves are out
of scope.

    removal-guard.py [--base REF] [--head REF]   check added lines (default origin/develop...HEAD)
    removal-guard.py --inventory [--head REF]    list every raw deletion in product code
"""
import argparse
import codecs
from pathlib import PurePosixPath
import re
import subprocess
import sys

# The helpers and this gate hold raw primitives by design.
EXEMPT_FILES = {
    "elastos/crates/elastos-common/src/owned_removal.rs",
    "scripts/lib/owned_removal.py",
    "scripts/lib/owned-removal.mjs",
    "scripts/removal-guard.py",
    "scripts/removal-guard-test.py",
}
# Not product code: docs, CI, vendored and example trees, tests and fixtures.
EXEMPT_DIRS = {".github", ".claude", ".cursor", ".githooks", "docs", "examples", "third-party",
               "third_party", "vendor", "node_modules", "tests", "test", "__tests__", "fixtures",
               "testdata", "benches"}
TEST_NAME = re.compile(r"(?:^tests?[._]|^test_|[._-]tests?\.|[._-]smoke\.|\.integration\.|[-_]repro\.)")
# .js/.ts in this repository run in a browser page, which deletes no host file.
LANGUAGES = {".rs": "rust", ".py": "python", ".sh": "shell", ".bash": "shell",
             ".mjs": "node", ".cjs": "node"}

# Calls, in any language: std/tokio fs::remove_*, unlink/unlinkat/rmdir
# (libc, nix, os, Path, fs), shutil.rmtree, os.remove, Node rm/rmSync/unlinkSync,
# and rm as the spawned program: ["rm", ...], spawn("rm", ...), Command::new("rm").
CALL = re.compile(
    r"(?<![\w$.])(?<!fn )remove_(?:file|dir_all|dir)\s*\("
    r"|(?<![\w$])(?<!fn )(?<!def )(?<!function )"
    r"(?:unlink|unlinkat|unlinkSync|rmdir|rmdirSync|rmtree|removedirs|rmSync|rm)\s*\("
    r"|\bos\.remove\s*\("
    r"""|(?:(?<!\.args\()\[|(?<!\.arg)\()\s*["'](?:/usr)?(?:/bin/)?(?:rm|rmdir|unlink)["']\s*[,\])]"""
)
# Shell commands: rm/rmdir/unlink in command position and find -delete.
SHELL = re.compile(r"(?<![\w./$-])(?:/usr)?(?:/bin/)?(?:rm|rmdir|unlink)(?=\s+\S)|\bfind\b.*\s-delete\b")
MARKER = re.compile(r"(?:#|//)\s*removal-ok:\s*\S.{7,}")
RUST_TEST_ATTRIBUTE = re.compile(r"^(\s*)#\[(?:cfg\((?:test|all\([^)]*\btest\b[^)]*\))\)|(?:tokio::)?test\b[^\]]*)\]")
HELPER = {
    "rust": "elastos_common::owned_removal::remove_owned(root, relative, expected)",
    "python": "remove_owned(root, relative, expected) from scripts/lib/owned_removal.py",
    "shell": 'python3 "${ROOT}/scripts/lib/owned_removal.py" ROOT RELATIVE [--expect DEV:INO]'
             " (embedded Python: remove_owned from scripts/lib/owned_removal.py)",
    "node": "removeOwned(root, relative, expected) from scripts/lib/owned-removal.mjs",
}


def git(*args, check=True):
    result = subprocess.run(["git", "-c", "core.quotePath=false", *args], encoding="utf-8",
                            errors="replace", stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if check and result.returncode:
        raise SystemExit(f"removal-guard: git {' '.join(args)} failed: {result.stderr.strip()}")
    return result


def unquote(path):
    if path.startswith('"'):
        raw = codecs.escape_decode(path[1:-1].encode())[0]
        return raw.decode("utf-8", "surrogateescape")
    return path


def language(path, first_line):
    """The product language of `path`, or None for tests, fixtures and non-code."""
    parts = PurePosixPath(path).parts
    if path in EXEMPT_FILES or any(part in EXEMPT_DIRS for part in parts[:-1]):
        return None
    name = parts[-1]
    if TEST_NAME.search(name):
        return None
    suffix = PurePosixPath(name).suffix
    if suffix:
        return LANGUAGES.get(suffix)
    if first_line.startswith("#!"):
        for word, found in (("python", "python"), ("node", "node"), ("bash", "shell"), ("sh", "shell")):
            if word in first_line:
                return found
    return None


def rust_test_lines(lines):
    """1-based line numbers inside #[cfg(test)] / #[test] items of rustfmt'd source."""
    inside = set()
    index = 0
    while index < len(lines):
        match = RUST_TEST_ATTRIBUTE.match(lines[index])
        if not match:
            index += 1
            continue
        indent, start = match[1], index
        item = index + 1
        while item < len(lines) and lines[item].strip().startswith(("#[", "///", "//")):
            item += 1
        end = item
        if item < len(lines) and not lines[item].rstrip().endswith(";") and \
                not re.search(r"\{.*\}\s*$", lines[item]):
            end = item + 1
            while end < len(lines) and not lines[end].startswith(indent + "}"):
                end += 1
        inside.update(range(start + 1, min(end, len(lines) - 1) + 2))
        index = end + 1
    return inside


def rust_test_module(path, read, depth=0):
    """True when `path` is a module file declared, directly or through its
    parent modules, inside #[cfg(test)]."""
    file = PurePosixPath(path)
    if file.name in ("lib.rs", "main.rs") or depth > 16:
        return False
    stem = file.parent.name if file.name == "mod.rs" else file.stem
    directory = file.parent.parent if file.name == "mod.rs" else file.parent
    parents = [directory / "mod.rs", directory / "lib.rs", directory / "main.rs",
               directory.parent / (directory.name + ".rs")]
    declaration = re.compile(rf"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(stem)}\s*;")
    for parent in parents:
        text = read(str(parent))
        if text is None:
            continue
        lines = text.split("\n")
        for number, line in enumerate(lines, 1):
            if declaration.match(line):
                return number in rust_test_lines(lines) or rust_test_module(str(parent), read, depth + 1)
    return False


def comment_only(line, lang):
    text = line.strip()
    if text.startswith("//") or (text.startswith("#") and not text.startswith(("#[", "#!"))):
        return True
    return lang in ("rust", "node") and (text.startswith("/*") or text == "*" or text.startswith(("* ", "*/")))


def raw_deletion(line, lang):
    return bool(CALL.search(line) or (lang == "shell" and SHELL.search(line)))


def marked(lines, number):
    if MARKER.search(lines[number - 1]):
        return True
    return number >= 2 and lines[number - 2].strip().startswith(("#", "//")) and \
        bool(MARKER.search(lines[number - 2]))


class Reader:
    """File text at one revision, read once through one `git cat-file --batch`."""

    def __init__(self, revision):
        self.revision, self.cache = git("rev-parse", "--verify", f"{revision}^{{commit}}").stdout.strip(), {}
        self.batch = subprocess.Popen(["git", "cat-file", "--batch"], stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE)

    def __call__(self, path):
        if path not in self.cache:
            self.batch.stdin.write(f"{self.revision}:{path}\n".encode())
            self.batch.stdin.flush()
            header = self.batch.stdout.readline().decode().split()
            text = None
            if len(header) == 3:
                data = self.batch.stdout.read(int(header[2]) + 1)[:-1]
                if header[1] == "blob":
                    text = data.decode("utf-8", "replace")
            self.cache[path] = text
        return self.cache[path]

    def close(self):
        self.batch.stdin.close()
        self.batch.wait()


def findings(path, numbers, read):
    """(line, lang, text, marked) for each raw deletion on `numbers` of product file `path`."""
    text = read(path)
    if text is None:
        return []
    lines = text.split("\n")
    lang = language(path, lines[0] if lines else "")
    if lang is None or (lang == "rust" and rust_test_module(path, read)):
        return []
    tests = rust_test_lines(lines) if lang == "rust" else set()
    found = []
    for number in sorted(numbers):
        if number > len(lines) or number in tests:
            continue
        line = lines[number - 1]
        if comment_only(line, lang) or not raw_deletion(line, lang):
            continue
        found.append((number, lang, line.strip(), marked(lines, number)))
    return found


def added_lines(base, head):
    diff = git("diff", "-U0", "--no-color", "--no-ext-diff", "--src-prefix=a/", "--dst-prefix=b/",
               f"{base}...{head}").stdout
    added, path, number, header = {}, None, 0, False
    for line in diff.split("\n"):
        if line.startswith("diff --git "):
            path, header = None, True
        elif header and line.startswith("+++ "):
            target = line[4:]
            path = None if target == "/dev/null" else unquote(target)[2:]
        elif line.startswith("@@"):
            header = False
            number = int(re.match(r"@@ -\S+ \+(\d+)", line)[1])
        elif path and not header and line.startswith("+"):
            added.setdefault(path, set()).add(number)
            number += 1
    return added


def check(base, head):
    if git("rev-parse", "--verify", "--quiet", f"{base}^{{commit}}", check=False).returncode:
        print(f"removal-guard: base {base} is unavailable; fetch it "
              "(git fetch origin +refs/heads/develop:refs/remotes/origin/develop)", file=sys.stderr)
        return 2
    read = Reader(head)
    refused = []
    for path, numbers in sorted(added_lines(base, head).items()):
        for number, lang, text, ok in findings(path, numbers, read):
            if not ok:
                refused.append((path, number, lang, text))
    read.close()
    for path, number, lang, text in refused:
        comment = "//" if lang in ("rust", "node") else "#"
        print(f"{path}:{number}: raw deletion added to product code: {text}\n"
              f"  fix: call {HELPER[lang]}, or end the line with "
              f"'{comment} removal-ok: <why this exact path cannot reach outside what this code created>'")
    if refused:
        print(f"removal-guard: {len(refused)} raw deletion(s) refused "
              "(exact owned paths only, every parent checked without following links)", file=sys.stderr)
        return 1
    print(f"removal-guard: no unguarded deletion added in {base}...{head}")
    return 0


def inventory(head):
    read = Reader(head)
    files = git("ls-tree", "-r", "--name-only", "-z", head).stdout.split("\0")
    counts = {}
    for path in filter(None, files):
        if PurePosixPath(path).suffix not in LANGUAGES and "." in PurePosixPath(path).name:
            continue
        text = read(path)
        if not text:
            continue
        for number, lang, line, ok in findings(path, range(1, text.count("\n") + 2), read):
            fd_relative = bool(re.search(r"unlinkat|dir_fd\s*=", line))
            state = "marked" if ok else "fd-relative" if fd_relative else "raw"
            total = counts.setdefault(lang, {"raw": 0, "fd-relative": 0, "marked": 0})
            total[state] += 1
            print(f"{path}:{number}: {lang}: {state}: {line}")
    read.close()
    for lang, total in sorted(counts.items()):
        print(f"removal-guard inventory: {lang}: {sum(total.values())} call(s): "
              + ", ".join(f"{count} {state}" for state, count in total.items()))
    return 0


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n", 1)[0])
    parser.add_argument("--base", default="origin/develop")
    parser.add_argument("--head", default="HEAD")
    parser.add_argument("--inventory", action="store_true")
    args = parser.parse_args()
    return inventory(args.head) if args.inventory else check(args.base, args.head)


if __name__ == "__main__":
    sys.exit(main())
