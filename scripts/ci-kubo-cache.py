#!/usr/bin/env python3
"""Prepare only verified Kubo inputs for CI caches; pins stay with their owner."""
import argparse
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
from time import sleep

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("upstream", ROOT / "scripts/release-upstream-input.py")
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)


def inputs(platform):
    recipes = json.loads((ROOT / "scripts/release-upstream-recipes.json").read_bytes())["recipes"]
    recipe, = [r for r in recipes if r["component"] == "kubo" and r["platform"] == platform]
    sources = [recipe["source"], *(item["source"] for item in
               [*recipe["license"]["files"], *recipe.get("notices", [])])]
    return recipe["version"], sources


def filename(source):
    algorithm, expected = upstream.checksum_parts(source["checksum"])
    return f"{algorithm}-{expected}"


def fetch(source, cache):
    cache = upstream.directory(cache)
    destination = cache / filename(source)
    if destination.exists() or destination.is_symlink():
        # Always validate a restore, even when actions/cache reports an exact hit.
        return upstream.cached_input(source, cache)
    # Each attempt has a wall-clock bound, including redirects and slow reads.
    # Only transport failures retry; checksum and policy errors fail immediately.
    for attempt in range(3):
        with tempfile.TemporaryDirectory(prefix=".kubo-fetch-", dir=cache) as directory:
            try:
                result = subprocess.run(
                    [sys.executable, str(Path(__file__).resolve()), "--fetch-one", directory],
                    input=json.dumps(source), text=True,
                    timeout=20 if source["max_bytes"] <= 1024**2 else 90, check=False)
                if result.returncode == 0:
                    verified = upstream.cached_input(source, Path(directory))
                    os.replace(verified, destination)
                    return destination
                if result.returncode != 75:
                    raise ValueError("Kubo input verification failed")
            except subprocess.TimeoutExpired:
                print(f"Kubo download attempt {attempt + 1} timed out", file=sys.stderr)
        if attempt < 2:
            sleep(5)
    raise ValueError("Kubo download failed after three bounded attempts")


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--fetch-one":
        try:
            upstream.cached_input(json.load(sys.stdin), Path(sys.argv[2]))
        except (OSError, http.client.HTTPException) as error:
            print(f"Kubo transport failed: {error}", file=sys.stderr)
            return 75
        except ValueError as error:
            print(f"Kubo input refused: {error}", file=sys.stderr)
            return 1
        return 0
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("key", "fetch"))
    parser.add_argument("platform", choices=("linux-amd64", "linux-arm64", "darwin-arm64"))
    parser.add_argument("cache", type=Path)
    args = parser.parse_args()
    version, sources = inputs(args.platform)
    if args.mode == "key":
        paths = [str(args.cache / filename(source)) for source in sources]
        # Licence changes also invalidate the input cache, independent of components.json.
        licences = upstream.canonical(sources[1:])
        key = f"kubo-{version}-{args.platform}-{filename(sources[0])}-{hashlib.sha256(licences).hexdigest()}"
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"key={key}\npaths<<KUBO_PATHS\n" + "\n".join(paths) + "\nKUBO_PATHS\n")
    else:
        for source in sources:
            fetch(source, args.cache)
        print("Verified every Kubo cache input against its recorded pins")
    return 0


if __name__ == "__main__":
    sys.exit(main())
