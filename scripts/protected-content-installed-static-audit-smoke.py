#!/usr/bin/env python3
import hashlib
import json
import os
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
AUDIT = "protected-content-installed-static-audit.py"
PLATFORM = "linux-amd64"
PROFILE = "protected-fixture"
CANONICAL = {
    "protected-content-protect-provider": "protect",
    "media-provider": "media",
    "custody-provider": "custody",
    "protected-content-decrypt-provider": "protected-content-decrypt",
}
PROVISIONAL = ("drm-provider", "rights-provider", "key-provider", "decrypt-provider")
REQUIRED = (
    "chain-provider",
    "protected-content-protect-provider",
    "media-provider",
    "custody-provider",
    "protected-content-decrypt-provider",
)
ALL_COMPONENTS = (*REQUIRED, "kubo", "ipfs-provider", *PROVISIONAL)


def sha256(data):
    return "sha256:" + hashlib.sha256(data).hexdigest()


def write_file(path, data, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    path.chmod(mode)


def run(command, *, cwd=None, env=None, check=True):
    return subprocess.run(
        command,
        cwd=cwd,
        env=env,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=check,
    )


def provider_runtime(name):
    if name in CANONICAL:
        return {
            "role": "provider",
            "substrate": "native",
            "runtime_abi": "elastos.provider-stdio/v1",
            "execution": "native-provider",
            "provides": CANONICAL[name],
            "runtime_only": True,
        }
    if name == "chain-provider":
        provides = "elastos://chain/*"
    elif name == "ipfs-provider":
        provides = "elastos://ipfs/*"
    else:
        return None
    return {
        "role": "provider",
        "substrate": "native",
        "runtime_abi": "elastos.provider-stdio/v1",
        "execution": "native-provider",
        "provides": provides,
    }


def manifest(binary_bytes):
    external = {}
    for name in ALL_COMPONENTS:
        data = binary_bytes[name]
        component = {
            "install_path": f"bin/{name}",
            "platforms": {
                PLATFORM: {
                    "release_path": f"{name}-{PLATFORM}",
                    "install_path": f"bin/{name}",
                    "checksum": sha256(data),
                }
            },
        }
        runtime = provider_runtime(name)
        if runtime is not None:
            component["provider_runtime"] = runtime
        external[name] = component
    return {
        "schema": "elastos.components/v1",
        "external": external,
        "capsules": {},
        "profiles": {
            PROFILE: {
                "components": list(ALL_COMPONENTS),
            }
        },
    }


def write_json(path, value, mode=0o600):
    write_file(path, (json.dumps(value, indent=2) + "\n").encode(), mode)


def tree_snapshot(root):
    result = {}
    for path in sorted((root, *root.rglob("*"))):
        metadata = path.lstat()
        relative = path.relative_to(root).as_posix() or "."
        entry = {
            "mode": stat.S_IMODE(metadata.st_mode),
            "size": metadata.st_size,
            "type": stat.S_IFMT(metadata.st_mode),
        }
        if stat.S_ISREG(metadata.st_mode):
            entry["sha256"] = sha256(path.read_bytes())
        elif stat.S_ISLNK(metadata.st_mode):
            entry["target"] = os.readlink(path)
        result[relative] = entry
    return result


class Fixture:
    def __init__(self, root):
        self.root = root
        self.source = root / "source"
        self.install = root / "stable-install"
        self.data = self.install / "data"
        self.runtime = self.install / "bin/elastos"
        self.binary_bytes = {
            name: f"fixture-{name}\n".encode() for name in ALL_COMPONENTS
        }
        self.runtime_bytes = b"fixture-runtime\n"
        self.secrets = (
            str(root),
            "https://private-rpc.invalid",
            "credential-private-value",
            "did:key:private-endpoint",
        )
        self.create()

    def create(self):
        scripts = self.source / "scripts"
        scripts.mkdir(parents=True)
        for name in (
            AUDIT,
            "components-release-integrity-check.py",
            "installed-provider-verify.sh",
        ):
            shutil.copy2(ROOT / "scripts" / name, scripts / name)
        (self.source / ".gitignore").write_text("**/target/\n", encoding="utf-8")
        write_json(self.source / "components.json", manifest(self.binary_bytes), 0o644)
        run(["git", "init", "--quiet"], cwd=self.source)
        run(["git", "config", "user.name", "Static Audit Smoke"], cwd=self.source)
        run(["git", "config", "user.email", "static-audit@invalid"], cwd=self.source)
        run(["git", "add", "."], cwd=self.source)
        run(["git", "commit", "--quiet", "-m", "static audit fixture"], cwd=self.source)

        write_json(self.data / "components.json", manifest(self.binary_bytes), 0o600)
        for name, data in self.binary_bytes.items():
            write_file(self.data / "bin" / name, data, 0o755)
        for name in (*REQUIRED, "ipfs-provider"):
            write_file(self.source / "elastos/target/release" / name, self.binary_bytes[name], 0o755)
        write_file(self.runtime, self.runtime_bytes, 0o755)
        write_file(self.source / "elastos/target/release/elastos", self.runtime_bytes, 0o755)
        self.write_private_state()

    def write_private_state(self):
        protected = self.data / "protected-content"
        media = protected / "media-provider"
        tools = media / "tools"
        staging = media / "staging"
        custody = protected / "custody-provider/inactive"
        for directory in (self.data, protected, media, tools, staging, custody):
            directory.mkdir(parents=True, exist_ok=True)
            directory.chmod(0o700)
        ffmpeg = tools / "ffmpeg"
        ffprobe = tools / "ffprobe"
        write_file(ffmpeg, b"fixture-ffmpeg\n", 0o500)
        write_file(ffprobe, b"fixture-ffprobe\n", 0o500)
        write_json(
            media / "config.json",
            {
                "schema": "elastos.protected-content.media-provider-config/v1",
                "ffmpeg_path": str(ffmpeg.resolve()),
                "ffprobe_path": str(ffprobe.resolve()),
                "staging_root": str(staging.resolve()),
                "output_profile": "browser_fmp4_h264_v1",
                "timeout_ms": 3_600_000,
                "max_stdio_bytes": 1 << 20,
                "max_input_bytes": 1 << 30,
                "max_output_part_bytes": 64 << 20,
                "max_duration_secs": 1_800,
                "max_source_width": 3_840,
                "max_source_height": 2_160,
                "max_source_fps": 60,
                "max_segment_count": 512,
                "max_total_output_bytes": 2 << 30,
            },
        )
        write_json(
            protected / "chain-provider.json",
            {"rpc": self.secrets[1], "credential": self.secrets[2]},
        )
        write_json(
            protected / "custody-composition.json",
            {"endpoint": self.secrets[3]},
        )

    def command(self, profile=PROFILE, data=None, runtime=None, role="custody-node"):
        return [
            "python3",
            str(self.source / "scripts" / AUDIT),
            "--source-root",
            str(self.source),
            "--installed-data-root",
            str(data or self.data),
            "--installed-runtime",
            str(runtime or self.runtime),
            "--platform",
            PLATFORM,
            "--profile",
            profile,
            "--role",
            role,
        ]

    def audit(self, expected_ok, profile=PROFILE, data=None, runtime=None, role="custody-node"):
        before = tree_snapshot(self.root)
        result = run(
            self.command(profile, data=data, runtime=runtime, role=role), check=False
        )
        after = tree_snapshot(self.root)
        if before != after:
            raise AssertionError("audit mutated the inspected fixture")
        receipt = json.loads(result.stdout)
        if (result.returncode == 0) != expected_ok:
            raise AssertionError((result.returncode, receipt, result.stderr))
        if receipt.get("ready_for_active_proof") is not expected_ok:
            raise AssertionError(receipt)
        if len(result.stdout.encode()) > 32 * 1024:
            raise AssertionError("receipt exceeds 32 KiB")
        combined = result.stdout + result.stderr
        for secret in self.secrets:
            if secret in combined:
                raise AssertionError(f"receipt leaked private value: {secret!r}")
        return receipt


def class_values(receipt, name):
    return (receipt.get("findings") or {}).get(name) or []


def require_finding(receipt, category, name):
    if name not in class_values(receipt, category):
        raise AssertionError((category, name, receipt))


def main():
    with tempfile.TemporaryDirectory() as directory:
        fixture = Fixture(Path(directory))
        receipt = fixture.audit(True)
        if receipt.get("schema") != "elastos.protected-content.installed-static-audit/v1":
            raise AssertionError(receipt)
        if receipt.get("static_ok") is not True:
            raise AssertionError(receipt)
        active = receipt.get("active_path") or {}
        if active.get("declared_mode") != "pre_cutover_coexistence":
            raise AssertionError(receipt)
        if active.get("status") != "active_proof_pending":
            raise AssertionError(receipt)
        if set(active.get("provisional_selected") or []) != set(PROVISIONAL):
            raise AssertionError(receipt)
        pending = class_values(receipt, "active_installed_proof_prerequisites")
        for name in (
            "provider_startup_and_registration",
            "replication_and_repair",
            "mint_buy_play",
        ):
            if name not in pending:
                raise AssertionError(receipt)
        if receipt.get("product_ready") is True:
            raise AssertionError("static audit claimed product readiness")

        home_receipt = fixture.audit(True, role="home")
        home_required = set(
            (home_receipt.get("canonical_installation") or {}).get("required") or []
        )
        if not {"kubo", "ipfs-provider"}.issubset(home_required):
            raise AssertionError(home_receipt)

        media = fixture.data / "bin/media-provider"
        media_bytes = media.read_bytes()
        media.write_bytes(b"mismatch\n")
        mismatch_receipt = fixture.audit(False)
        require_finding(
            mismatch_receipt,
            "source_static_artifact_failures",
            "artifact:media-provider",
        )
        require_finding(
            mismatch_receipt,
            "source_static_artifact_failures",
            "artifact_parity:media-provider",
        )
        write_file(media, media_bytes, 0o755)

        media.unlink()
        require_finding(
            fixture.audit(False),
            "source_static_artifact_failures",
            "artifact:media-provider",
        )
        write_file(media, media_bytes, 0o755)

        require_finding(
            fixture.audit(False, profile="missing-profile"),
            "source_static_artifact_failures",
            "profile",
        )

        chain = fixture.data / "protected-content/chain-provider.json"
        chain.chmod(0o640)
        require_finding(
            fixture.audit(False),
            "operator_configuration_prerequisites",
            "chain_config",
        )
        chain.chmod(0o600)

        manifest_path = fixture.data / "components.json"
        installed_manifest = json.loads(manifest_path.read_text())
        original_media_info = installed_manifest["external"]["media-provider"][
            "platforms"
        ][PLATFORM].copy()
        installed_manifest["external"]["media-provider"]["platforms"][PLATFORM][
            "install_path"
        ] = "target/release/media-provider"
        write_json(manifest_path, installed_manifest)
        require_finding(
            fixture.audit(False),
            "source_static_artifact_failures",
            "binding:media-provider",
        )
        installed_manifest["external"]["media-provider"]["platforms"][
            PLATFORM
        ] = original_media_info
        write_json(manifest_path, installed_manifest)

        installed_manifest["external"]["duplicate-media"] = {
            "install_path": "bin/duplicate-media",
            "provider_runtime": provider_runtime("media-provider"),
            "platforms": {},
        }
        write_json(manifest_path, installed_manifest)
        require_finding(
            fixture.audit(False),
            "source_static_artifact_failures",
            "private_provider_metadata:media-provider",
        )
        del installed_manifest["external"]["duplicate-media"]
        write_json(manifest_path, installed_manifest)

        with tempfile.TemporaryDirectory(dir="/tmp") as unstable_directory:
            unstable_data = Path(unstable_directory) / "installed-data"
            shutil.copytree(fixture.data, unstable_data)
            unstable_before = tree_snapshot(unstable_data)
            receipt = fixture.audit(False, data=unstable_data)
            if unstable_before != tree_snapshot(unstable_data):
                raise AssertionError("disposable data-root audit mutated its fixture")
            require_finding(
                receipt,
                "source_static_artifact_failures",
                "installed_data_location",
            )

        with tempfile.TemporaryDirectory(dir="/tmp") as unstable_directory:
            unstable_runtime = Path(unstable_directory) / "elastos"
            write_file(unstable_runtime, fixture.runtime_bytes, 0o755)
            unstable_before = tree_snapshot(unstable_runtime.parent)
            receipt = fixture.audit(False, runtime=unstable_runtime)
            if unstable_before != tree_snapshot(unstable_runtime.parent):
                raise AssertionError("disposable Runtime audit mutated its fixture")
            require_finding(
                receipt,
                "source_static_artifact_failures",
                "installed_runtime_location",
            )

        for path, finding in (
            (chain, "chain_config"),
            (fixture.data / "protected-content/custody-composition.json", "custody_composition"),
            (fixture.data / "protected-content/media-provider/config.json", "media_config"),
        ):
            content = path.read_bytes()
            mode = stat.S_IMODE(path.stat().st_mode)
            path.unlink()
            require_finding(
                fixture.audit(False),
                "operator_configuration_prerequisites",
                finding,
            )
            write_file(path, content, mode)

        custody_state = fixture.data / "protected-content/custody-provider/inactive"
        custody_state.rmdir()
        require_finding(
            fixture.audit(False),
            "operator_configuration_prerequisites",
            "custody_state",
        )
        custody_state.mkdir()
        custody_state.chmod(0o700)

        exposed = fixture.data / "capsules/media-provider/capsule.json"
        write_json(exposed, {"resource": "elastos://media/prepare"})
        require_finding(
            fixture.audit(False),
            "source_static_artifact_failures",
            "private_provider_capsule_exposure:media-provider",
        )
        exposed.unlink()

        helper_env = os.environ.copy()
        helper_env.update(
            {
                "ELASTOS_DATA_DIR": str(fixture.data),
                "ELASTOS_COMPONENTS_JSON": str(manifest_path),
                "ELASTOS_SETUP_PLATFORM": PLATFORM,
            }
        )
        installed_manifest = json.loads(manifest_path.read_text())
        media_info = installed_manifest["external"]["media-provider"]["platforms"][PLATFORM]
        original_info = media_info.copy()
        media_info.clear()
        media_info.update(
            {
                "strategy": "local-copy",
                "source": "fixture-source",
                "install_path": "bin/media-provider",
            }
        )
        write_json(manifest_path, installed_manifest)
        helper = fixture.source / "scripts/installed-provider-verify.sh"
        legacy = run(
            ["bash", str(helper), "media-provider"],
            cwd=fixture.source,
            env=helper_env,
            check=False,
        )
        strict = run(
            ["bash", str(helper), "--require-verified", "media-provider"],
            cwd=fixture.source,
            env=helper_env,
            check=False,
        )
        if legacy.returncode != 0 or strict.returncode == 0:
            raise AssertionError((legacy.stderr, strict.stderr))
        media_info.clear()
        media_info.update(original_info)
        write_json(manifest_path, installed_manifest)

        fixture.audit(True)

    print("PASS protected-content installed static audit smoke")


if __name__ == "__main__":
    main()
