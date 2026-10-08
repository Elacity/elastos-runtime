#!/usr/bin/env bash
# Build-only prerequisite: wrap one pinned Kubo input as a licensed capsule.
# Consumers install the release capsule through Carrier.
# Usage: seed-kubo-cache.sh <cache-dir> <data-dir> <platform> [--verify-installed]
set -euo pipefail
umask 077

if [[ $# -lt 3 || $# -gt 4 || ( $# -eq 4 && "$4" != --verify-installed ) ]]; then
    echo "usage: $0 <cache-dir> <data-dir> <platform> [--verify-installed]" >&2
    exit 2
fi
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python3 - "$REPO_ROOT" "$1" "$2" "$3" "${4:-}" <<'PY'
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tarfile
import tempfile

root, cache, data = map(Path, sys.argv[1:4])
platform, verify = sys.argv[4], sys.argv[5] == '--verify-installed'
spec = importlib.util.spec_from_file_location('release_upstream', root / 'scripts/release-upstream-input.py')
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)
recipes = json.loads((root / 'scripts/release-upstream-recipes.json').read_bytes())['recipes']
selected = [r for r in recipes if r['component'] == 'kubo' and r['platform'] == platform]
if len(selected) != 1:
    raise SystemExit('Kubo requires one build recipe for the selected platform')
recipe = selected[0]
cache, data = upstream.directory(cache), upstream.directory(data)
if verify:
    # Existing source Homes get a local proof against recorded upstream bytes.
    # Verification has no download path, including separately pinned licences.
    notices = [*recipe['license']['files'], *recipe.get('notices', [])]
    for source in [recipe['source'], *(item['source'] for item in notices)]:
        algorithm, expected = upstream.checksum_parts(source['checksum'])
        upstream.regular(cache / f'{algorithm}-{expected}')
with tempfile.TemporaryDirectory(prefix='.kubo-package-', dir=cache) as temporary:
    receipt = upstream.package(recipe, cache, Path(temporary))
    archive = Path(temporary) / receipt['release_path']
    packages = upstream.directory(cache / 'capsules')
    saved = packages / receipt['release_path']
    if saved.exists() or saved.is_symlink():
        if upstream.digest(saved) != receipt['checksum'][7:]:
            raise SystemExit('Cached Kubo capsule differs from its build recipe')
    else:
        shutil.copyfile(archive, saved)
        saved.chmod(0o600)
    with tarfile.open(archive, 'r:gz') as bundle:
        binary = bundle.extractfile(recipe['extract_path']).read()
        target = upstream.directory(data / 'bin') / 'kubo'
        if verify:
            if upstream.regular(target).read_bytes() != binary:
                raise SystemExit('Installed Kubo differs from the cached recipe input')
        else:
            if target.exists() or target.is_symlink():
                upstream.regular(target)
            fd, pending = tempfile.mkstemp(prefix='.kubo-', dir=target.parent)
            try:
                with os.fdopen(fd, 'wb') as stream:
                    stream.write(binary)
                os.chmod(pending, 0o700)
                os.replace(pending, target)
            finally:
                Path(pending).unlink(missing_ok=True)
        capsule = upstream.directory(data / 'capsules/kubo')
        for member in bundle.getmembers():
            name = member.name.removeprefix(recipe['root'] + '/')
            if name == member.name or not member.isfile():
                raise SystemExit('Kubo capsule contains an unsupported member')
            if name != '_elastos_object.json':
                upstream.relative(name)
            destination = capsule / name
            upstream.directory(destination.parent)
            content = bundle.extractfile(member).read()
            if destination.exists() or destination.is_symlink():
                if upstream.regular(destination).read_bytes() == content:
                    continue
                if verify:
                    raise SystemExit('Installed Kubo capsule differs from its recipe')
            destination.write_bytes(content)
            destination.chmod(member.mode & 0o700)
    marker = capsule / '.elastos-artifact-sha256'
    if marker.exists() or marker.is_symlink():
        upstream.regular(marker)
    marker.write_text(receipt['checksum'].removeprefix('sha256:') + '\n')
    marker.chmod(0o600)
    for record in (packages / (receipt['release_path'] + '.json'),
                   upstream.directory(data / 'receipts') / 'kubo-build.json'):
        if record.exists() or record.is_symlink():
            upstream.regular(record)
        record.write_text(json.dumps(receipt, sort_keys=True) + '\n')
        record.chmod(0o600)
print('[seed-kubo] verified licensed Kubo capsule and native prerequisite')
PY
