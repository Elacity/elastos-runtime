#!/usr/bin/env python3
import hashlib
import importlib.util
import json
import shutil
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
STAMP_SCRIPT = ROOT / "scripts" / "stamp-source-home-capsule-metadata.py"
MANAGED_STATE_SCHEMA = "elastos.source-home.managed-capsules/v1"


def load_stamper():
    spec = importlib.util.spec_from_file_location("capsule_stamper", STAMP_SCRIPT)
    if spec is None or spec.loader is None:
        raise AssertionError(f"cannot load {STAMP_SCRIPT}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n", encoding="utf-8")


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def tree_hashes(root):
    return {
        path.relative_to(root).as_posix(): sha256(path)
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def projection_manifest():
    return {
        "schema": "elastos.capsule/v1",
        "name": "gba-emulator",
        "version": "0.1.0",
        "description": "Play GBA games",
        "role": "viewer",
        "type": "wasm",
        "runtime_abi": "elastos.runtime-projection/v1",
        "bus_contract": "elastos.runtime-projection/v1",
        "execution": "web-projection",
        "projections": ["web", "facts", "affordances", "gates"],
        "entrypoint": "browser/index.html",
        "interfaces": [
            {
                "id": "elastos.gba.emulator",
                "version": "0.5.0",
                "methods": [
                    {
                        "id": "game.open",
                        "risk": "launch",
                        "approval": "runtime_policy",
                        "audit": "event",
                        "resource": "elastos://content/asset",
                        "operation": "open",
                        "input_schema": {
                            "accepts": [
                                {
                                    "kind": "content_capsule",
                                    "role": "content",
                                    "type": "data",
                                    "viewer": "gba-emulator",
                                },
                                {"kind": "file", "extensions": [".gba"]},
                            ]
                        },
                    }
                ],
            }
        ],
        "permissions": {
            "storage": ["localhost://Users/self/.AppData/LocalHost/GBA/*"]
        },
        "requires": [
            {"name": "object-provider", "kind": "capsule"},
        ],
    }


def content_manifest(name, description, entrypoint, *, icon=None):
    manifest = {
        "schema": "elastos.capsule/v1",
        "name": name,
        "version": "0.1.0",
        "description": description,
        "role": "content",
        "type": "data",
        "entrypoint": entrypoint,
        "viewer": "gba-emulator",
        "interfaces": [
            {
                "id": "elastos.content.asset",
                "version": "0.5.0",
                "methods": [
                    {
                        "id": "asset.open",
                        "risk": "launch",
                        "approval": "runtime_policy",
                        "audit": "event",
                        "resource": "elastos://content/asset",
                        "operation": "open",
                    }
                ],
            }
        ],
    }
    if icon is not None:
        manifest["icon"] = icon
    return manifest


def make_source_capsules(root):
    emulator = root / "capsules" / "gba-emulator"
    write_json(emulator / "capsule.json", projection_manifest())
    (emulator / "browser").mkdir(parents=True)
    (emulator / "browser" / "index.html").write_text(
        "<main>GBA</main>\n", encoding="utf-8"
    )

    ucity = root / "capsules" / "gba-ucity"
    write_json(ucity / "capsule.json", content_manifest("gba-ucity", "uCity", "ucity.gba"))
    (ucity / "ucity.gba").write_bytes(b"gba-rom")

    nonogram = root / "capsules" / "gba-nonogram"
    write_json(
        nonogram / "capsule.json",
        content_manifest(
            "gba-nonogram",
            "Nonogram Advance",
            "nonogram.gba",
            icon="icons",
        ),
    )
    (nonogram / "nonogram.gba").write_bytes(b"nonogram-rom")
    (nonogram / "icons").mkdir(parents=True)
    (nonogram / "icons" / "icon.svg").write_text("<svg></svg>\n", encoding="utf-8")
    for size in [32, 64, 128, 256]:
        (nonogram / "icons" / f"icon-{size}.png").write_bytes(
            f"png-{size}".encode("utf-8")
        )

    provider = root / "capsules" / "object-provider"
    write_json(
        provider / "capsule.json",
        {
            "schema": "elastos.capsule/v1",
            "name": "object-provider",
            "version": "0.1.0",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "elastos://object/*",
        },
    )


def make_installed_capsules(root, data_dir):
    for name in ["gba-emulator", "gba-ucity", "gba-nonogram"]:
        shutil.copytree(root / "capsules" / name, data_dir / "capsules" / name)
    for name in ["chat-wasm", "old-managed", "user-capsule"]:
        capsule_dir = data_dir / "capsules" / name
        capsule_dir.mkdir(parents=True)
        write_json(
            capsule_dir / "capsule.json",
            {
                "schema": "elastos.capsule/v1",
                "name": name,
                "version": "0.1.0",
                "role": "app",
                "type": "wasm",
                "entrypoint": "app.wasm",
            },
        )
        (capsule_dir / "app.wasm").write_bytes(name.encode())
    stale_provider = data_dir / "capsules" / "object-provider"
    stale_provider.mkdir(parents=True)
    write_json(
        stale_provider / "capsule.json",
        {
            "schema": "elastos.capsule/v1",
            "name": "object-provider",
            "version": "0.0.1",
            "role": "provider",
            "type": "microvm",
            "entrypoint": "rootfs.ext4",
            "provides": "object://*",
        },
    )
    (stale_provider / "stale-rootfs.ext4").write_bytes(b"stale")


def make_kubo_rewritten_components(path):
    # This is the lossy shape produced when setup serializes components.json
    # before source-home applies its final capsule metadata stamp.
    write_json(
        path,
        {
            "external": {},
            "capsules": {
                "gba-emulator": {"cid": "", "sha256": "", "size": 0},
                "gba-ucity": {"cid": "", "sha256": "", "size": 0},
                "gba-nonogram": {"cid": "", "sha256": "", "size": 0},
            },
            "profiles": {
                "demo": {"components": ["gba-emulator", "gba-ucity", "gba-nonogram"]}
            },
        },
    )


def assert_setup_order():
    setup = (ROOT / "scripts" / "setup-source-home.sh").read_text(encoding="utf-8")
    main = setup.split('echo "[setup-source-home] repo:', 1)[1]
    ordered = [
        "stamp_source_home_components_manifest\n",
        "install_content_publish_backend\n",
        "install_app_capsules\n",
        "stamp_source_home_capsule_artifacts_manifest\n",
        'python3 "${ROOT}/scripts/components-release-integrity-check.py"',
    ]
    positions = [main.index(marker) for marker in ordered]
    if positions != sorted(positions):
        raise AssertionError(
            "components.json mutators must finish before the final capsule stamp and integrity check"
        )
    if 'manifest.setdefault("profiles", {})["source-home"]' not in setup:
        raise AssertionError("source-home setup must stamp the exact components it installs")
    integrity_call = main.split(
        'python3 "${ROOT}/scripts/components-release-integrity-check.py"', 1
    )[1]
    if "--profile source-home" not in integrity_call:
        raise AssertionError("source-home integrity must validate the stamped source-home profile")
    if "--profile demo" in integrity_call:
        raise AssertionError("source-home integrity must not require prebuilt-only demo artifacts")


def run_smoke():
    stamper = load_stamper()
    with tempfile.TemporaryDirectory() as temp:
        temp_root = Path(temp)
        root = temp_root / "repo"
        data_dir = temp_root / "data"
        components_path = data_dir / "components.json"
        managed_state_path = data_dir / "receipts" / "source-home-capsules.json"

        make_source_capsules(root)
        make_installed_capsules(root, data_dir)
        make_kubo_rewritten_components(components_path)
        write_json(
            managed_state_path,
            {
                "schema": MANAGED_STATE_SCHEMA,
                "capsules": ["old-managed"],
            },
        )

        passkey = data_dir / "ElastOS" / "System" / "Auth" / "passkey.json"
        user_data = data_dir / "Users" / "alice" / "document.txt"
        passkey.parent.mkdir(parents=True)
        user_data.parent.mkdir(parents=True)
        passkey.write_text('{"credential":"preserve-me"}\n', encoding="utf-8")
        user_data.write_text("preserve me\n", encoding="utf-8")
        protected_before = {str(path): sha256(path) for path in [passkey, user_data]}

        removed = stamper.finalize_source_home_capsules(
            components_path=components_path,
            data_dir=data_dir,
            root=root,
            platform="darwin-arm64",
            capsules=["gba-emulator", "gba-ucity", "gba-nonogram", "object-provider"],
            retired_capsules=["chat-wasm"],
            managed_state_path=managed_state_path,
        )

        if removed != ["chat-wasm", "old-managed"]:
            raise AssertionError(f"unexpected removed capsules: {removed}")
        for name in removed:
            if (data_dir / "capsules" / name).exists():
                raise AssertionError(f"managed inactive capsule still exists: {name}")
        if not (data_dir / "capsules" / "user-capsule").is_dir():
            raise AssertionError("unmanaged capsule must be preserved")
        installed_provider = data_dir / "capsules" / "object-provider"
        if sorted(path.name for path in installed_provider.iterdir()) != ["capsule.json"]:
            raise AssertionError("provider contract materialization retained stale payload files")
        if json.loads((installed_provider / "capsule.json").read_text()) != json.loads(
            (root / "capsules" / "object-provider" / "capsule.json").read_text()
        ):
            raise AssertionError("installed provider contract differs from source")
        if sha256(installed_provider / "capsule.json") != sha256(
            root / "capsules" / "object-provider" / "capsule.json"
        ):
            raise AssertionError("installed provider contract is not byte-identical to source")

        protected_after = {str(path): sha256(path) for path in [passkey, user_data]}
        if protected_before != protected_after:
            raise AssertionError("source-home finalization modified protected user state")

        components = json.loads(components_path.read_text(encoding="utf-8"))
        emulator = components["capsules"]["gba-emulator"]
        ucity = components["capsules"]["gba-ucity"]
        nonogram = components["capsules"]["gba-nonogram"]
        if emulator.get("runtime_abi") != "elastos.runtime-projection/v1":
            raise AssertionError("GBA emulator runtime ABI was not restored")
        if emulator.get("execution") != "web-projection":
            raise AssertionError("GBA emulator execution metadata was not restored")
        if emulator.get("projections") != ["web", "facts", "affordances", "gates"]:
            raise AssertionError("GBA emulator projections were not restored")
        if ucity.get("role") != "content" or ucity.get("type") != "data":
            raise AssertionError("uCity content role/type metadata was not restored")
        if ucity.get("viewer") != "gba-emulator":
            raise AssertionError("uCity viewer metadata was not restored")
        if (
            nonogram.get("role") != "content"
            or nonogram.get("type") != "data"
            or nonogram.get("viewer") != "gba-emulator"
        ):
            raise AssertionError("Nonogram content metadata was not restored")

        for name in ["gba-emulator", "gba-ucity", "gba-nonogram"]:
            source = root / "capsules" / name
            installed = data_dir / "capsules" / name
            if tree_hashes(source) != tree_hashes(installed):
                raise AssertionError(f"source-installed capsule tree mismatch: {name}")
        installed_nonogram = json.loads(
            (data_dir / "capsules" / "gba-nonogram" / "capsule.json").read_text(
                encoding="utf-8"
            )
        )
        if installed_nonogram.get("icon") != "icons":
            raise AssertionError("installed Nonogram icon manifest was not preserved")
        installed_emulator = json.loads(
            (data_dir / "capsules" / "gba-emulator" / "capsule.json").read_text(
                encoding="utf-8"
            )
        )
        accepts = installed_emulator["interfaces"][0]["methods"][0]["input_schema"][
            "accepts"
        ]
        if not any(item.get("extensions") == [".gba"] for item in accepts):
            raise AssertionError("installed GBA interface metadata was lost")

        managed = json.loads(managed_state_path.read_text(encoding="utf-8"))
        if managed != {
            "schema": MANAGED_STATE_SCHEMA,
            "capsules": ["gba-emulator", "gba-nonogram", "gba-ucity", "object-provider"],
        }:
            raise AssertionError(f"unexpected managed state: {managed}")

        unsafe_state = data_dir / "receipts" / "unsafe-managed-capsules.json"
        write_json(
            unsafe_state,
            {"schema": MANAGED_STATE_SCHEMA, "capsules": ["../Users"]},
        )
        try:
            stamper.remove_managed_inactive_capsules(data_dir, [], [], unsafe_state)
        except SystemExit:
            pass
        else:
            raise AssertionError("unsafe managed capsule names must fail closed")

        linked_capsule = data_dir / "capsules" / "linked-managed"
        linked_capsule.symlink_to(data_dir / "Users", target_is_directory=True)
        write_json(
            unsafe_state,
            {"schema": MANAGED_STATE_SCHEMA, "capsules": ["linked-managed"]},
        )
        try:
            stamper.remove_managed_inactive_capsules(data_dir, [], [], unsafe_state)
        except SystemExit:
            pass
        else:
            raise AssertionError("managed capsule symlinks must fail closed")
        if not user_data.is_file() or sha256(user_data) != protected_before[str(user_data)]:
            raise AssertionError("unsafe cleanup probe modified protected user data")

    assert_setup_order()
    print("PASS source-home capsule inventory smoke")


if __name__ == "__main__":
    run_smoke()
