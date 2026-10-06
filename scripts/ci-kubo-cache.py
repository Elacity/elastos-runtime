#!/usr/bin/env python3
"""Prepare only verified Kubo inputs for CI caches; pins stay with their owner."""
import argparse
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
from time import sleep
import urllib.error

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("upstream", ROOT / "scripts/release-upstream-input.py")
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)


def inputs(platform, custody=False):
    if custody:
        dockerfile = (ROOT / "deploy/custody-host/Dockerfile").read_text()
        version = re.search(r"(?m)^ARG KUBO_VERSION=(\S+)$", dockerfile)[1]
        pins = dict(re.findall(r'(amd64|arm64)\) kubo_sha256="([0-9a-f]{64})"', dockerfile))
        arch = platform.removeprefix("linux-")
        if platform not in ("linux-amd64", "linux-arm64"):
            raise ValueError("unsupported custody Kubo platform")
        filename = f"kubo_{version}_{platform}.tar.gz"
        source = {"url": f"https://dist.ipfs.tech/kubo/{version}/{filename}",
                  "checksum": "sha256:" + pins[arch], "max_bytes": 512 * 1024**2}
        mirror = {**source, "url": f"https://github.com/ipfs/kubo/releases/download/{version}/{filename}",
                  "redirect_hosts": ["release-assets.githubusercontent.com"]}
        return version, pins[arch], [(source, mirror)]
    recipes = json.loads((ROOT / "scripts/release-upstream-recipes.json").read_bytes())["recipes"]
    recipe, = [r for r in recipes if r["component"] == "kubo" and r["platform"] == platform]
    sources = [recipe["source"], *(item["source"] for item in
               [*recipe["license"]["files"], *recipe.get("notices", [])])]
    return recipe["version"], recipe["source"]["sha256"], [(s,) for s in sources]


def filename(source):
    algorithm, expected = upstream.checksum_parts(source["checksum"])
    return f"{algorithm}-{expected}"


def fetch(sources, cache):
    cache = upstream.directory(cache)
    source = sources[0]
    destination = cache / filename(source)
    if destination.exists() or destination.is_symlink():
        # Always validate a restore, even when actions/cache reports an exact hit.
        return upstream.cached_input(source, cache)
    # Each attempt has a wall-clock bound, including redirects and slow reads.
    # Only transport failures retry; checksum and policy errors fail immediately.
    for attempt in range(3):
        route = sources[min(attempt, len(sources) - 1)]
        with tempfile.TemporaryDirectory(prefix=".kubo-fetch-", dir=cache) as directory:
            try:
                result = subprocess.run(
                    [sys.executable, str(Path(__file__).resolve()), "--fetch-one", directory],
                    input=json.dumps(route), text=True,
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
        except (urllib.error.URLError, TimeoutError, ConnectionError, http.client.IncompleteRead) as error:
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
    parser.add_argument("--custody", action="store_true")
    args = parser.parse_args()
    version, sha256, sources = inputs(args.platform, args.custody)
    if args.mode == "key":
        paths = [str(args.cache / filename(s[0])) for s in sources]
        # Licence changes also invalidate the input cache, independent of components.json.
        licences = upstream.canonical([s[0] for s in sources[1:]])
        key = f"kubo-{version}-{args.platform}-{sha256}-{hashlib.sha256(licences).hexdigest()}"
        with open(os.environ["GITHUB_OUTPUT"], "a") as output:
            output.write(f"key={key}\npaths<<KUBO_PATHS\n" + "\n".join(paths) + "\nKUBO_PATHS\n")
    else:
        for routes in sources:
            fetch(routes, args.cache)
        if args.custody:
            # The image builder uses a separate UID to read these public bytes.
            args.cache.chmod(0o755)
            for routes in sources:
                (args.cache / filename(routes[0])).chmod(0o644)
        print("Verified every Kubo cache input against its recorded pins")
    return 0


if __name__ == "__main__":
    sys.exit(main())
