#!/usr/bin/env python3
"""Prepare checksum-pinned upstream capsules for a native release worker."""

import argparse
import copy
import hashlib
import importlib.util
import json
from pathlib import Path


SOURCE_ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("upstream_input", SOURCE_ROOT / "scripts/release-upstream-input.py")
upstream = importlib.util.module_from_spec(spec)
spec.loader.exec_module(upstream)
RECIPE_FILE = SOURCE_ROOT / "scripts/release-upstream-recipes.json"


def selected_recipes(platform):
    document = json.loads(RECIPE_FILE.read_text())
    if document.get("schema") != "elastos.release-upstream-recipes/v1":
        raise ValueError("unsupported upstream recipe inventory")
    selected = [recipe for recipe in document["recipes"] if recipe["platform"] in (platform, "*")]
    names = [recipe["component"] for recipe in selected]
    if len(names) != len(set(names)):
        raise ValueError("duplicate upstream component for the selected platform")
    return selected


def resolved_recipe(original, llama_arm64_bundle=None):
    recipe = copy.deepcopy(original)
    sources = [recipe["source"], *[item["source"] for item in recipe["license"]["files"]],
               *[item["source"] for item in recipe.get("notices", [])]]
    for source in sources:
        if "path" not in source:
            continue
        if source["path"] == "@llama-arm64-bundle":
            if llama_arm64_bundle is None:
                raise ValueError("qualified Linux ARM64 llama bundle is required")
            source["path"] = str(Path(llama_arm64_bundle).resolve(strict=True))
        else:
            relative = upstream.relative(source["path"])
            path = SOURCE_ROOT / relative
            upstream.regular(path)
            source["path"] = str(path.resolve(strict=True))
    return recipe


def prepare(platform, cache, output, llama_arm64_bundle=None):
    recipes = selected_recipes(platform)
    output = upstream.directory(output)
    external, receipts = {}, []
    for original in recipes:
        recipe = resolved_recipe(original, llama_arm64_bundle)
        receipt = upstream.package(recipe, cache, output)
        fields = ("release_path", "checksum", "size", "extract_path", "install_path", "binary_path")
        info = {key: receipt[key] for key in fields if key in receipt}
        metadata = receipt["capsule_metadata"]
        external[recipe["component"]] = {
            "platforms": {recipe["platform"]: info},
            "capsule_metadata": {"role": "content", "type": "data", "install_path": metadata["install_path"],
                                 "platforms": {recipe["platform"]: metadata}},
        }
        receipt["recipe_sha256"] = hashlib.sha256(upstream.canonical(original)).hexdigest()
        receipts.append(receipt)
    result = {"external": external}
    # This receipt belongs to native preparation, not the installed manifest.
    receipt_path = output / "upstream-input.json"
    with receipt_path.open("x") as stream:
        json.dump({"schema": "elastos.release-upstream-assets/v1", "platform": platform,
                   "recipes_sha256": hashlib.sha256(RECIPE_FILE.read_bytes()).hexdigest(),
                   "capsules": receipts}, stream, indent=2)
        stream.write("\n")
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", choices=("linux-amd64", "linux-arm64", "darwin-arm64"), required=True)
    parser.add_argument("--cache", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--llama-arm64-bundle", type=Path)
    args = parser.parse_args()
    print(json.dumps(prepare(args.platform, args.cache, args.output, args.llama_arm64_bundle)))


if __name__ == "__main__":
    main()
