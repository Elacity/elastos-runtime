#!/usr/bin/env python3
"""Refuse process-global PATH, HOME and XDG_* mutation in Rust sources.

Cargo runs a binary's tests as threads of one process. A test that sets PATH
to "" makes a concurrent test's `Command::new("python3")` fail with NotFound;
HOME/XDG_* moves every concurrent `dirs::data_dir()`. Pass the value into the
function under test instead. A line that must mutate one carries
`// global-env-ok: <reason>`.

Covers std::env::set_var/remove_var and scoped guards called as `X::set("HOME", ..)`.
"""
import re
import subprocess
import sys
import tempfile
from pathlib import Path

CALL = re.compile(r'(?:\bset_var|\bremove_var|::set|::remove)\s*\(\s*"(PATH|HOME|XDG_[A-Z0-9_]+)"')
MARKER = re.compile(r"//\s*global-env-ok:\s*\S")


def violations(path, text):
    lines = text.splitlines()
    found = []
    for match in CALL.finditer(text):
        first = text.count("\n", 0, match.start())
        last = text.count("\n", 0, match.end())
        if not any(MARKER.search(lines[index]) for index in range(first, last + 1)):
            found.append(f"{path}:{first + 1}: sets process-global {match.group(1)}; pass it into the "
                         "code under test, or mark the line `// global-env-ok: <reason>`")
    return found


def check(root):
    files = subprocess.check_output(["git", "ls-files", "-z", "--", "*.rs"], cwd=root, text=True)
    found = []
    for name in filter(None, files.split("\0")):
        found.extend(violations(name, (root / name).read_text(encoding="utf-8", errors="replace")))
    return found


def self_test():
    cases = {
        'std::env::set_var("PATH", "");': 1,
        'unsafe { std::env::remove_var("HOME") };': 1,
        'let _x = ScopedEnvVar::set("XDG_DATA_HOME", tmp);': 1,
        'std::env::set_var(\n    "PATH",\n    "",\n);': 1,
        'std::env::set_var("PATH", ""); // global-env-ok: child-only test binary': 0,
        'std::env::set_var(\n    "PATH", // global-env-ok: own process\n    "",\n);': 0,
        'std::env::set_var("PATH", ""); // global-env-ok:': 1,
        'std::env::set_var("ELASTOS_TOKEN", "x");': 0,
        'Command::new("tar").env("PATH", "").env_remove("HOME");': 0,
        'let p = std::env::var_os("PATH");': 0,
    }
    for source, expected in cases.items():
        got = len(violations("fixture.rs", source))
        assert got == expected, f"{source!r}: expected {expected} violation(s), got {got}"
    # The tree walk reads tracked Rust files only, from a real repository.
    with tempfile.TemporaryDirectory() as scratch:
        root = Path(scratch)
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        (root / "a.rs").write_text('fn t() { std::env::set_var("HOME", "/x"); }\n')
        (root / "untracked.rs").write_text('fn t() { std::env::set_var("PATH", ""); }\n')
        subprocess.run(["git", "add", "a.rs"], cwd=root, check=True)
        found = check(root)
        assert len(found) == 1 and found[0].startswith("a.rs:1:"), found
    print("[rust-global-env] self-test OK")


def main():
    if sys.argv[1:] == ["--self-test"]:
        self_test()
        return 0
    if sys.argv[1:]:
        print("usage: rust-global-env-check.py [--self-test]", file=sys.stderr)
        return 2
    root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
    found = check(root)
    for line in found:
        print(line, file=sys.stderr)
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main())
