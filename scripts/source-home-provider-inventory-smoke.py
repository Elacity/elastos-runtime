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
PUBLISH_RELEASE = ROOT / "scripts" / "publish-release.sh"
SCHEMA = "elastos.source-home-provider-inventory-smoke/v1"

RUNTIME_ONLY_PROVIDERS = (
    "custody-provider",
    "media-provider",
    "protected-content-protect-provider",
    "protected-content-decrypt-provider",
)
REQUIRED_PROVIDER_RUNTIMES = (
    "custody-provider",
    "localhost-provider",
    "media-provider",
    "model-provider",
    "protected-content-protect-provider",
    "protected-content-decrypt-provider",
)
RETAINED_RELEASE_PROVIDERS = (
    "operator-drive-adapter",
    "drm-provider",
    "rights-provider",
    "key-provider",
    "decrypt-provider",
)


def assert_provider_names_and_loops(setup_text: str) -> None:
    provider_fn = setup_text.split("provider_runtime_names() {", 1)[1].split(
        "}\n\nsource_home_helper_binary_names", 1
    )[0]
    if 'runtime = component.get("provider_runtime")' not in provider_fn:
        raise AssertionError("provider runtime inventory must derive from components.json")

    helper_fn = setup_text.split("source_home_helper_binary_names() {", 1)[1].split(
        "}\n\nsource_home_binary_names", 1
    )[0]
    for name in RUNTIME_ONLY_PROVIDERS:
        if f"        {name}" in helper_fn:
            raise AssertionError(
                f"source_home_helper_binary_names() must not duplicate Runtime provider {name}"
            )

    build_loop = 'source_home_binary_names | while IFS= read -r provider; do'
    if setup_text.count(build_loop) < 2:
        raise AssertionError(
            "setup-source-home must build and install its derived binary inventory"
        )

    main = setup_text.split('echo "[setup-source-home] repo:', 1)[1]
    build_index = main.index('echo "[setup-source-home] build native provider binaries"')
    prerequisite_index = main.index("prepare_media_provider_prerequisite\n")
    install_index = main.index('echo "[setup-source-home] install native providers and stamp manifest"')
    if not build_index < prerequisite_index < install_index:
        raise AssertionError(
            "media-provider prerequisite must run after provider build and before provider install"
        )

    prerequisite_fn = setup_text.split("prepare_media_provider_prerequisite() {", 1)[1].split(
        "\n}\n\ninstall_content_publish_backend() {", 1
    )[0]
    if "setup --with media-provider --prerequisites-only" not in prerequisite_fn:
        raise AssertionError("source-home must use the setup media prerequisite preflight")
    if '--media-tools-dir "${SETUP_SOURCE_HOME_MEDIA_TOOLS_DIR}"' not in prerequisite_fn:
        raise AssertionError("source-home must pass its explicit media tools directory")

    if 'SOURCE_HOME_BINARY_NAMES_JSON="${SOURCE_HOME_BINARY_NAMES_JSON}"' not in setup_text:
        raise AssertionError("source-home stamp must receive SOURCE_HOME_BINARY_NAMES_JSON")

    for private_state in (
        "protected-content/chain-provider.json",
        "protected-content/custody-composition.json",
        "node-custody-secret",
        "evidence_rpc_urls",
    ):
        if private_state in setup_text:
            raise AssertionError(
                f"source-home packaging must not create protected operator state: {private_state}"
            )


def shell_array_entries(text: str, name: str) -> list[str]:
    body = text.split(f"{name}=(", 1)[1].split("\n)", 1)[0]
    return [
        line.strip()
        for line in body.splitlines()
        if line.strip() and not line.lstrip().startswith("#")
    ]


def assert_release_support_inventory() -> None:
    publish_text = PUBLISH_RELEASE.read_text(encoding="utf-8")
    support = shell_array_entries(publish_text, "SUPPORT_BINARY_ASSETS")
    for provider in RUNTIME_ONLY_PROVIDERS:
        if support.count(provider) != 1:
            raise AssertionError(
                f"release support inventory must include {provider} exactly once"
            )
    for private_state in (
        "protected-content/chain-provider.json",
        "protected-content/custody-composition.json",
        "node-custody-secret",
        "evidence_rpc_urls",
    ):
        if private_state in publish_text:
            raise AssertionError(
                f"release packaging must not create protected operator state: {private_state}"
            )

    support_assets = publish_text.split("build_supported_direct_assets() {", 1)[1].split(
        "\n}\n\nbuild_platform_independent_direct_assets() {", 1
    )[0]
    if "updates_json='{}'" not in support_assets:
        raise AssertionError("normal support assets must remain top-level direct updates")
    if "updates_json='{\"external\":{}}'" in support_assets:
        raise AssertionError("normal support assets must not add an external component wrapper")

    provider_assets = publish_text.split(
        "build_platform_independent_provider_capsule_metadata_assets() {", 1
    )[1].split("\n}\n\nmerge_direct_assets() {", 1)[0]
    if "supported-provider-contracts-universal" not in provider_assets:
        raise AssertionError("provider capsule metadata must use its own staging directory")
    if "record_provider_capsule_metadata_asset" not in provider_assets:
        raise AssertionError("provider capsule metadata must remain under external components")

    publisher_handoff = publish_text.split(
        "# Copy first-party support assets for Carrier-served setup fetches.", 1
    )[1].split("# Copy capsule artifacts for Carrier serving", 1)[0]
    provider_handoff_loop = publisher_handoff.split(
        'for f in "${TMPDIR}/supported-provider-contracts-universal"/*; do', 1
    )
    if len(provider_handoff_loop) != 2:
        raise AssertionError("publisher artifacts must include provider capsule metadata archives")
    provider_handoff_body = provider_handoff_loop[1].split("done", 1)[0]
    if '"${artifact_dir}/$(basename "$f")"' not in provider_handoff_body:
        raise AssertionError("publisher metadata archive names must match their stamped release paths")


def assert_external_metadata(components: dict) -> None:
    capsules = components["capsules"]
    external = components["external"]
    for name in REQUIRED_PROVIDER_RUNTIMES:
        if name in RUNTIME_ONLY_PROVIDERS and name in capsules:
            raise AssertionError(f"components.json must not list {name} under capsules")
        entry = external.get(name)
        if not isinstance(entry, dict):
            raise AssertionError(f"components.json missing external metadata for {name}")
        if entry.get("install_path") != f"bin/{name}":
            raise AssertionError(f"{name} install_path mismatch")
        source_manifests = (
            ROOT / "capsules" / name / "Cargo.toml",
            ROOT / "elastos" / "capsules" / name / "Cargo.toml",
            ROOT / "elastos" / "tools" / name / "Cargo.toml",
        )
        if not any(path.is_file() for path in source_manifests):
            raise AssertionError(f"{name} source manifest is unavailable for clean builds")
        platforms = entry.get("platforms") or {}
        for platform in ("linux-amd64", "linux-arm64", "darwin-arm64"):
            info = platforms.get(platform)
            if not isinstance(info, dict):
                raise AssertionError(f"{name} missing {platform} platform metadata")
            if info.get("install_path") != f"bin/{name}":
                raise AssertionError(f"{name} {platform} install_path mismatch")
            if info.get("release_path") != f"{name}-{platform}":
                raise AssertionError(f"{name} {platform} release_path mismatch")

    for name, provides in (
        ("custody-provider", "custody"),
        ("media-provider", "media"),
        ("model-provider", "elastos://model/*"),
        ("protected-content-protect-provider", "protect"),
        ("protected-content-decrypt-provider", "protected-content-decrypt"),
    ):
        runtime = external[name].get("provider_runtime") or {}
        expected_runtime = {
            "role": "provider",
            "substrate": "native",
            "runtime_abi": "elastos.provider-stdio/v1",
            "execution": "native-provider",
            "provides": provides,
        }
        if name in RUNTIME_ONLY_PROVIDERS:
            expected_runtime["runtime_only"] = True
        if runtime != expected_runtime:
            raise AssertionError(f"{name} runtime contract mismatch")


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


def run_retained_provider_stamp_smoke(setup_text: str, components: dict) -> None:
    stamp = setup_text.split("stamp_source_home_components_manifest() {", 1)[1]
    stamp = stamp.split("python3 - <<'PY'\n", 1)[1].split("\nPY\n}", 1)[0]
    with tempfile.TemporaryDirectory() as temp_dir:
        root = Path(temp_dir)
        data_dir = root / "data"
        bin_dir = data_dir / "bin"
        bin_dir.mkdir(parents=True)
        source = root / "source-components.json"
        installed = data_dir / "components.json"
        names = ("shell", "model-provider", "kubo", *RETAINED_RELEASE_PROVIDERS)
        manifest = {
            "schema": components["schema"],
            "capsules": {},
            "external": {name: json.loads(json.dumps(components["external"][name])) for name in names},
        }
        previous = json.loads(json.dumps(manifest))
        for name in names:
            payload = f"{name}-installed\n".encode()
            (bin_dir / name).write_bytes(payload)
            if name == "kubo":
                continue  # Kubo's pin is for an archive, not its installed binary.
            prior = previous["external"][name]["platforms"]["linux-amd64"]
            prior.update(
                checksum="sha256:" + hashlib.sha256(payload).hexdigest(),
                size=len(payload),
                cid=f"bafy-{name}-prior",
            )
            current = manifest["external"][name]["platforms"]["linux-amd64"]
            current.update(checksum="", size=0, cid="")
        source.write_text(json.dumps(manifest))
        installed.write_text(json.dumps(previous))
        env = {**os.environ,
               "COMPONENTS_SRC": str(source), "COMPONENTS_DEST": str(installed),
               "DATA_DIR": str(data_dir), "SETUP_PLATFORM": "linux-amd64",
               "SOURCE_HOME_BINARY_NAMES_JSON": json.dumps(["model-provider"]),
               "APP_CAPSULES_JSON": "[]", "SOURCE_HOME_KUBO_INSTALLED": "0"}

        def stamp_once() -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                ["python3", "-c", stamp], env=env, text=True, capture_output=True,
                cwd=ROOT,
            )

        result = stamp_once()
        if result.returncode:
            raise AssertionError(f"source-home stamp failed: {result.stderr}")
        stamped = json.loads(installed.read_text())
        for name in RETAINED_RELEASE_PROVIDERS:
            assert stamped["external"][name]["platforms"]["linux-amd64"] == previous["external"][name]["platforms"]["linux-amd64"]
            assert name in stamped["profiles"]["source-home"]["components"]
        assert stamped["external"]["kubo"] == manifest["external"]["kubo"]
        assert "kubo" not in stamped["profiles"]["source-home"]["components"]
        first = installed.read_bytes()
        assert stamp_once().returncode == 0
        assert installed.read_bytes() == first

        (bin_dir / "drm-provider").write_bytes(b"changed provider bytes")
        result = stamp_once()
        assert result.returncode != 0 and "does not match its prior" in result.stderr
        assert installed.read_bytes() == first

        (bin_dir / "drm-provider").write_bytes(b"drm-provider-installed\n")
        stripped = json.loads(installed.read_text())
        stripped["external"]["drm-provider"]["platforms"]["linux-amd64"].update(
            checksum="", size=0, cid="",
        )
        installed.write_text(json.dumps(stripped))
        stripped_bytes = installed.read_bytes()
        result = stamp_once()
        assert result.returncode != 0 and "checksum is unavailable" in result.stderr
        assert installed.read_bytes() == stripped_bytes


def main() -> None:
    setup_text = SETUP.read_text(encoding="utf-8")
    components = json.loads(COMPONENTS.read_text(encoding="utf-8"))

    assert_provider_names_and_loops(setup_text)
    assert_release_support_inventory()
    assert_external_metadata(components)
    run_integrity_smoke(components)
    run_retained_provider_stamp_smoke(setup_text, components)

    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "ok": True,
                "providers": [*REQUIRED_PROVIDER_RUNTIMES],
            }
        )
    )


if __name__ == "__main__":
    main()
