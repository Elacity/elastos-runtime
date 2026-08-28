#!/usr/bin/env python3
import hashlib
import json
import os
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
SETUP = ROOT / "scripts" / "setup-source-home.sh"
COMPONENTS = ROOT / "components.json"
INTEGRITY = ROOT / "scripts" / "components-release-integrity-check.py"
SCHEMA = "elastos.source-home-provider-inventory-smoke/v1"

PROTECTED_CONTENT_HELPERS = (
    "custody-provider",
    "protected-content-protect-provider",
    "protected-content-decrypt-provider",
)
REQUIRED_PROVIDER_RUNTIMES = ("localhost-provider", "model-provider")


def assert_provider_names_and_loops(setup_text: str) -> None:
    provider_fn = setup_text.split("provider_runtime_names() {", 1)[1].split(
        "}\n\nsource_home_helper_binary_names", 1
    )[0]
    if 'runtime = component.get("provider_runtime")' not in provider_fn:
        raise AssertionError("provider runtime inventory must derive from components.json")

    helper_fn = setup_text.split("source_home_helper_binary_names() {", 1)[1].split(
        "}\n\nsource_home_binary_names", 1
    )[0]
    for name in PROTECTED_CONTENT_HELPERS:
        needle = f"        {name}"
        if needle not in helper_fn:
            raise AssertionError(f"source_home_helper_binary_names() missing {name}")

    build_loop = 'source_home_binary_names | while IFS= read -r provider; do'
    if setup_text.count(build_loop) < 2:
        raise AssertionError(
            "setup-source-home must build and install its derived binary inventory"
        )

    main = setup_text.split('echo "[setup-source-home] repo:', 1)[1]
    build_index = main.index('echo "[setup-source-home] build native provider binaries"')
    install_index = main.index('echo "[setup-source-home] install native providers and stamp manifest"')
    if build_index >= install_index:
        raise AssertionError("provider build must precede provider install/stamp")

    if 'SOURCE_HOME_BINARY_NAMES_JSON="${SOURCE_HOME_BINARY_NAMES_JSON}"' not in setup_text:
        raise AssertionError("source-home stamp must receive SOURCE_HOME_BINARY_NAMES_JSON")


def assert_external_metadata(components: dict) -> None:
    capsules = components["capsules"]
    external = components["external"]
    for name in (*PROTECTED_CONTENT_HELPERS, *REQUIRED_PROVIDER_RUNTIMES):
        if name in PROTECTED_CONTENT_HELPERS and name in capsules:
            raise AssertionError(f"components.json must not list {name} under capsules")
        entry = external.get(name)
        if not isinstance(entry, dict):
            raise AssertionError(f"components.json missing external metadata for {name}")
        if entry.get("install_path") != f"bin/{name}":
            raise AssertionError(f"{name} install_path mismatch")
        platforms = entry.get("platforms") or {}
        for platform in ("linux-amd64", "linux-arm64", "darwin-arm64"):
            info = platforms.get(platform)
            if not isinstance(info, dict):
                raise AssertionError(f"{name} missing {platform} platform metadata")
            if info.get("install_path") != f"bin/{name}":
                raise AssertionError(f"{name} {platform} install_path mismatch")
            if info.get("release_path") != f"{name}-{platform}":
                raise AssertionError(f"{name} {platform} release_path mismatch")

    model_runtime = external["model-provider"].get("provider_runtime") or {}
    expected_runtime = {
        "role": "provider",
        "substrate": "native",
        "runtime_abi": "elastos.provider-stdio/v1",
        "execution": "native-provider",
        "provides": "elastos://model/*",
    }
    if model_runtime != expected_runtime:
        raise AssertionError("model-provider runtime contract mismatch")


def run_integrity_smoke(components: dict) -> None:
    with tempfile.TemporaryDirectory() as temp_dir:
        temp_root = Path(temp_dir)
        manifest_path = temp_root / "components.json"
        data_dir = temp_root / "data"
        bin_dir = data_dir / "bin"
        bin_dir.mkdir(parents=True)

        host_names = [
            "shell",
            *REQUIRED_PROVIDER_RUNTIMES,
            *PROTECTED_CONTENT_HELPERS,
        ]
        manifest = {
            "schema": components["schema"],
            "capsules": {},
            "external": {name: json.loads(json.dumps(components["external"][name])) for name in host_names},
            "profiles": {
                "protected-content-provider-smoke": {
                    "description": "Focused source-home protected-content provider packaging smoke",
                    "components": host_names,
                }
            },
        }

        for name in host_names:
            payload = f"{name}-smoke\n".encode("utf-8")
            target = bin_dir / name
            target.write_bytes(payload)
            os.chmod(target, 0o755)
            info = manifest["external"][name]["platforms"]["darwin-arm64"]
            info["checksum"] = "sha256:" + hashlib.sha256(payload).hexdigest()
            info["size"] = len(payload)

        manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")

        subprocess.run(
            [
                "python3",
                str(INTEGRITY),
                "--manifest",
                str(manifest_path),
                "--platform",
                "darwin-arm64",
                "--profile",
                "protected-content-provider-smoke",
                "--source-root",
                str(ROOT),
                "--source-home-data-dir",
                str(data_dir),
            ],
            check=True,
            cwd=ROOT,
        )


def main() -> None:
    setup_text = SETUP.read_text(encoding="utf-8")
    components = json.loads(COMPONENTS.read_text(encoding="utf-8"))

    assert_provider_names_and_loops(setup_text)
    assert_external_metadata(components)
    run_integrity_smoke(components)

    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "ok": True,
                "providers": [
                    *REQUIRED_PROVIDER_RUNTIMES,
                    *PROTECTED_CONTENT_HELPERS,
                ],
            }
        )
    )


if __name__ == "__main__":
    main()
