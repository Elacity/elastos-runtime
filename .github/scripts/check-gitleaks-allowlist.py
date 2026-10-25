#!/usr/bin/env python3
"""Fails unless every gitleaks allow-list entry says why it is safe (supply-chain gate).

    python3 check-gitleaks-allowlist.py [repo-root]

- .gitleaksignore: every fingerprint line needs a comment directly above it (no blank line in
  between; one comment block may cover a group of consecutive fingerprints) with at least ten
  letters of text, i.e. a reason, not just a separator line.
- .gitleaks.toml: every [allowlist], [[allowlists]] and per-rule allow-list needs a non-empty
  description.
- Inline `gitleaks:allow` markers in tracked source files (not Markdown or the gitleaks files,
  which only document the marker) must carry a reason on the same line (`gitleaks:allow reason ...`).
Standard library only (Python 3.11+ for tomllib).
"""
import pathlib
import re
import subprocess
import sys
import tomllib

root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
errors = []

ignore = root / ".gitleaksignore"
if ignore.exists():
    has_reason = False
    for n, raw in enumerate(ignore.read_text().splitlines(), 1):
        line = raw.strip()
        if not line:
            has_reason = False
        elif line.startswith("#"):
            if sum(c.isalpha() for c in line) >= 10:
                has_reason = True
        elif not has_reason:
            errors.append(f"{ignore}:{n}: allow-list entry has no reason comment directly above it")

config = root / ".gitleaks.toml"
if config.exists():
    data = tomllib.loads(config.read_text())
    tables = []
    if "allowlist" in data:
        tables.append(("[allowlist]", data["allowlist"]))
    tables += [(f"[[allowlists]] #{i}", t) for i, t in enumerate(data.get("allowlists", []), 1)]
    for rule in data.get("rules", []):
        rid = rule.get("id", "?")
        if "allowlist" in rule:
            tables.append((f"rule {rid} [rules.allowlist]", rule["allowlist"]))
        tables += [(f"rule {rid} [[rules.allowlists]] #{i}", t) for i, t in enumerate(rule.get("allowlists", []), 1)]
    for name, table in tables:
        if not str(table.get("description", "")).strip():
            errors.append(f"{config}: {name} has no description stating why it is safe")

try:
    out = subprocess.run(["git", "-C", str(root), "grep", "-n", "-I", "gitleaks:allow", "--",
                          ":!.gitleaks.toml", ":!.gitleaksignore", ":!*.md",
                          ":!.github/scripts/check-gitleaks-allowlist.py"],
                         capture_output=True, text=True).stdout
except FileNotFoundError:
    out = ""
for hit in out.splitlines():
    tail = hit.split("gitleaks:allow", 1)[1]
    if sum(c.isalpha() for c in re.sub(r"[*/#\-]+", " ", tail)) < 10:
        errors.append(f"{hit.split(':', 2)[0]}:{hit.split(':', 2)[1]}: inline gitleaks:allow without a reason")

for e in errors:
    print(f"::error::{e}")
print(f"gitleaks allow-list check: {len(errors)} entr{'y' if len(errors) == 1 else 'ies'} without a reason")
sys.exit(1 if errors else 0)
