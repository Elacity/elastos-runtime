#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
from pathlib import Path


SCHEMA = "elastos.protected-content.installed-static-audit/v1"
MAX_MANIFEST_BYTES = 4 * 1024 * 1024
MAX_CONFIG_BYTES = 64 * 1024
MAX_MEDIA_CONFIG_BYTES = 8 * 1024
MAX_ARTIFACT_BYTES = 2 * 1024 * 1024 * 1024
MAX_HELPER_OUTPUT_BYTES = 256 * 1024
MAX_RECEIPT_BYTES = 32 * 1024
MAX_PROFILE_COMPONENTS = 256
ID_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,63}$")
GIT_ID_RE = re.compile(r"^[0-9a-f]{40,64}$")
SOURCE_PARITY_EXEMPT = {"kubo"}

CANONICAL = {
    "protected-content-protect-provider": "protect",
    "media-provider": "media",
    "custody-provider": "custody",
    "protected-content-decrypt-provider": "protected-content-decrypt",
}
PROVISIONAL = ("drm-provider", "rights-provider", "key-provider", "decrypt-provider")
ROLE_REQUIRED = {
    "home": (
        "chain-provider",
        "kubo",
        "ipfs-provider",
        "protected-content-protect-provider",
        "media-provider",
        "protected-content-decrypt-provider",
    ),
    "custody-node": (
        "chain-provider",
        "protected-content-protect-provider",
        "media-provider",
        "custody-provider",
        "protected-content-decrypt-provider",
    ),
}
ACTIVE_PROOF_PREREQUISITES = (
    "runtime_config_acceptance",
    "signed_custody_validation",
    "live_chain_evidence",
    "provider_startup_and_registration",
    "replication_and_repair",
    "mint_buy_play",
)
MEDIA_SCHEMA = "elastos.protected-content.media-provider-config/v1"
MEDIA_PROFILE = "browser_fmp4_h264_v1"
MEDIA_LIMITS = {
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
}
MEDIA_FIELDS = {
    "schema",
    "ffmpeg_path",
    "ffprobe_path",
    "staging_root",
    "output_profile",
    *MEDIA_LIMITS,
}


class Findings:
    def __init__(self):
        self.static = set()
        self.operator = set()

    def artifact(self, name):
        self.static.add(name)

    def config(self, name):
        self.operator.add(name)


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Read installed protected-content artifacts and emit one redacted static audit receipt."
    )
    parser.add_argument("--source-root", required=True)
    parser.add_argument("--installed-data-root", required=True)
    parser.add_argument("--installed-runtime", required=True)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--profile", required=True)
    parser.add_argument("--role", required=True, choices=tuple(ROLE_REQUIRED))
    return parser.parse_args(argv)


def canonical_input(value, finding, findings):
    try:
        return Path(value).expanduser().resolve(strict=True)
    except (OSError, RuntimeError):
        findings.artifact(finding)
        return None


def run_read_only(command, cwd, env=None, timeout=30):
    safe_env = os.environ.copy() if env is None else env.copy()
    safe_env["GIT_OPTIONAL_LOCKS"] = "0"
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            env=safe_env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if max(len(result.stdout), len(result.stderr)) > MAX_HELPER_OUTPUT_BYTES:
        return None
    return result


def read_json(path, limit, finding, findings):
    try:
        metadata = path.lstat()
        if (
            not stat.S_ISREG(metadata.st_mode)
            or stat.S_ISLNK(metadata.st_mode)
            or metadata.st_size <= 0
            or metadata.st_size > limit
        ):
            raise ValueError
        value = json.loads(path.read_text(encoding="utf-8"))
        if not isinstance(value, dict):
            raise ValueError
        return value
    except (OSError, UnicodeError, json.JSONDecodeError, ValueError):
        findings.artifact(finding)
        return None


def sha256_file(path, finding, findings, executable=False):
    try:
        metadata = path.lstat()
        if (
            not stat.S_ISREG(metadata.st_mode)
            or stat.S_ISLNK(metadata.st_mode)
            or metadata.st_size <= 0
            or metadata.st_size > MAX_ARTIFACT_BYTES
            or (executable and metadata.st_mode & 0o111 == 0)
        ):
            raise ValueError
        digest = hashlib.sha256()
        with path.open("rb") as handle:
            for chunk in iter(lambda: handle.read(1024 * 1024), b""):
                digest.update(chunk)
        return "sha256:" + digest.hexdigest()
    except (OSError, ValueError):
        findings.artifact(finding)
        return None


def owner_only(
    path, *, directory=False, limit=MAX_CONFIG_BYTES, executable=False, exact_mode=None
):
    try:
        metadata = path.lstat()
    except OSError:
        return False
    expected_type = stat.S_ISDIR if directory else stat.S_ISREG
    return (
        expected_type(metadata.st_mode)
        and not stat.S_ISLNK(metadata.st_mode)
        and metadata.st_uid == os.geteuid()
        and metadata.st_mode & 0o077 == 0
        and (exact_mode is None or stat.S_IMODE(metadata.st_mode) == exact_mode)
        and (directory or (metadata.st_nlink == 1 and 0 < metadata.st_size <= limit))
        and (not executable or metadata.st_mode & 0o100 != 0)
    )


def git_identity(source_root, findings):
    values = []
    for revision in ("HEAD", "HEAD^{tree}"):
        result = run_read_only(
            ["git", "-C", str(source_root), "rev-parse", "--verify", revision],
            source_root,
            timeout=10,
        )
        value = (
            result.stdout.decode("ascii", errors="ignore").strip().lower()
            if result is not None and result.returncode == 0
            else ""
        )
        if not GIT_ID_RE.fullmatch(value):
            findings.artifact("source_git_identity")
            return None, None, False
        values.append(value)
    status = run_read_only(
        [
            "git",
            "-C",
            str(source_root),
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
        ],
        source_root,
        timeout=15,
    )
    clean = status is not None and status.returncode == 0 and not status.stdout
    if not clean:
        findings.artifact("source_tree_clean")
    return values[0], values[1], clean


def platform_info(component, platform):
    aliases = {
        "linux-amd64": "x86_64-linux",
        "x86_64-linux": "linux-amd64",
        "linux-arm64": "aarch64-linux",
        "aarch64-linux": "linux-arm64",
        "darwin-arm64": "macos-arm64",
        "macos-arm64": "darwin-arm64",
    }
    platforms = component.get("platforms") if isinstance(component, dict) else None
    if not isinstance(platforms, dict):
        return None
    for key in (platform, aliases.get(platform), "*"):
        if key and isinstance(platforms.get(key), dict):
            return platforms[key]
    return None


def contains_disposable_binding(value, source_root):
    if not isinstance(value, str) or not value:
        return False
    normalized = value.replace("\\", "/")
    return (
        normalized.startswith(("/tmp/", "/private/tmp/", "/var/tmp/"))
        or "/target/" in normalized
        or "/node_modules/" in normalized
        or normalized == str(source_root)
        or normalized.startswith(str(source_root) + "/")
    )


def disposable_install_location(path, source_root):
    try:
        canonical = path.resolve(strict=True)
        text = canonical.as_posix()
        return (
            text == "/tmp"
            or text.startswith("/tmp/")
            or text == "/private/tmp"
            or text.startswith("/private/tmp/")
            or text == "/var/tmp"
            or text.startswith("/var/tmp/")
            or "target" in canonical.parts
            or canonical == source_root
            or canonical.is_relative_to(source_root)
        )
    except (OSError, RuntimeError, ValueError):
        return True


def component_path(manifest, data_root, name, platform, source_root, findings):
    external = manifest.get("external")
    component = external.get(name) if isinstance(external, dict) else None
    info = platform_info(component, platform)
    if not isinstance(component, dict) or not isinstance(info, dict):
        findings.artifact(f"component:{name}")
        return None, component
    install_path = info.get("install_path") or component.get("install_path")
    source_binding = info.get("source")
    relative = Path(install_path) if isinstance(install_path, str) else Path()
    if (
        install_path != f"bin/{name}"
        or relative.is_absolute()
        or ".." in relative.parts
        or "target" in relative.parts
        or contains_disposable_binding(source_binding, source_root)
    ):
        findings.artifact(f"binding:{name}")
        return None, component
    path = data_root.joinpath(*relative.parts)
    try:
        if path.resolve(strict=True).parent != data_root.joinpath("bin").resolve(strict=True):
            raise ValueError
    except (OSError, RuntimeError, ValueError):
        findings.artifact(f"binding:{name}")
        return None, component
    return path, component


def verify_canonical_metadata(manifest, findings):
    external = manifest.get("external")
    external = external if isinstance(external, dict) else {}
    target_owners = {target: [] for target in CANONICAL.values()}
    metadata_ok = {}
    for name, component in external.items():
        runtime = component.get("provider_runtime") if isinstance(component, dict) else None
        if isinstance(runtime, dict) and runtime.get("provides") in target_owners:
            target_owners[runtime["provides"]].append(name)
    for name, target in CANONICAL.items():
        runtime = (external.get(name) or {}).get("provider_runtime")
        valid = runtime == {
            "role": "provider",
            "substrate": "native",
            "runtime_abi": "elastos.provider-stdio/v1",
            "execution": "native-provider",
            "provides": target,
            "runtime_only": True,
        }
        if not valid or target_owners[target] != [name]:
            findings.artifact(f"private_provider_metadata:{name}")
        metadata_ok[name] = valid and target_owners[target] == [name]
    return metadata_ok


def audit_capsule_exposure(source_root, data_root, manifest, findings):
    capsules = manifest.get("capsules")
    capsules = capsules if isinstance(capsules, dict) else {}
    for name, target in CANONICAL.items():
        exposed = name in capsules
        for root in (source_root / "capsules", source_root / "elastos/capsules", data_root / "capsules"):
            exposed = exposed or (root / name / "capsule.json").exists()
        for entry in capsules.values():
            if not isinstance(entry, dict):
                continue
            public_value = " ".join(
                str(entry.get(key) or "") for key in ("provides", "resource", "uri")
            )
            exposed = exposed or target in public_value or f"elastos://{target}" in public_value
        if exposed:
            findings.artifact(f"private_provider_capsule_exposure:{name}")


def delegate_checks(source_root, data_root, platform, profile, providers, findings):
    manifest_checker = source_root / "scripts/components-release-integrity-check.py"
    provider_checker = source_root / "scripts/installed-provider-verify.sh"
    if not manifest_checker.is_file() or manifest_checker.is_symlink():
        findings.artifact("manifest_integrity_helper")
    else:
        result = run_read_only(
            [
                "python3",
                str(manifest_checker),
                "--manifest",
                str(data_root / "components.json"),
                "--platform",
                platform,
                "--profile",
                profile,
                "--source-root",
                str(source_root),
                "--source-home-data-dir",
                str(data_root),
            ],
            source_root,
        )
        if result is None or result.returncode != 0:
            findings.artifact("installed_manifest_integrity")
    if not provider_checker.is_file() or provider_checker.is_symlink():
        findings.artifact("provider_integrity_helper")
        return
    env = os.environ.copy()
    env.update(
        {
            "ELASTOS_DATA_DIR": str(data_root),
            "ELASTOS_COMPONENTS_JSON": str(data_root / "components.json"),
            "ELASTOS_SETUP_PLATFORM": platform,
        }
    )
    for name in providers:
        result = run_read_only(
            ["bash", str(provider_checker), "--require-verified", name],
            source_root,
            env=env,
        )
        if result is None or result.returncode != 0:
            findings.artifact(f"artifact:{name}")


def source_artifact(source_root, name):
    candidates = (
        source_root / "elastos/target/release" / name,
        source_root / "capsules" / name / "target/release" / name,
        source_root / "elastos/capsules" / name / "target/release" / name,
        source_root / "elastos/tools" / name / "target/release" / name,
    )
    return [candidate for candidate in candidates if candidate.exists()]


def audit_artifacts(source_root, data_root, runtime_path, manifest, platform, names, findings):
    installed = {}
    source = {}
    parity = {}
    for name in names:
        path, _ = component_path(manifest, data_root, name, platform, source_root, findings)
        installed_hash = sha256_file(path, f"artifact:{name}", findings, executable=True) if path else None
        candidates = [] if name in SOURCE_PARITY_EXEMPT else source_artifact(source_root, name)
        source_hashes = {
            sha256_file(candidate, f"source_artifact:{name}", findings, executable=True)
            for candidate in candidates
        }
        source_hashes.discard(None)
        if name in SOURCE_PARITY_EXEMPT:
            source_hash = None
        elif len(source_hashes) != 1:
            findings.artifact(f"source_artifact:{name}")
            source_hash = None
        else:
            source_hash = next(iter(source_hashes))
        if installed_hash:
            installed[name] = installed_hash
        if source_hash:
            source[name] = source_hash
        if installed_hash and source_hash:
            parity[name] = installed_hash == source_hash
            if not parity[name]:
                findings.artifact(f"artifact_parity:{name}")

    runtime_hash = sha256_file(runtime_path, "installed_runtime", findings, executable=True)
    source_runtime = source_root / "elastos/target/release/elastos"
    source_runtime_hash = sha256_file(
        source_runtime, "source_runtime", findings, executable=True
    )
    runtime_parity = runtime_hash is not None and runtime_hash == source_runtime_hash
    if runtime_hash and source_runtime_hash and not runtime_parity:
        findings.artifact("runtime_parity")
    return {
        "installed_sha256": installed,
        "source_sha256": source,
        "parity": parity,
        "runtime": {
            "installed_sha256": runtime_hash,
            "source_sha256": source_runtime_hash,
            "parity": runtime_parity,
        },
    }


def audit_media(data_root, findings):
    protected = data_root / "protected-content"
    media = protected / "media-provider"
    tools = media / "tools"
    staging = media / "staging"
    for path, name in (
        (protected, "protected_content_root"),
        (media, "media_root"),
        (tools, "media_tools_root"),
        (staging, "media_staging_root"),
    ):
        if not owner_only(path, directory=True, exact_mode=0o700):
            findings.config(name)
    config_path = media / "config.json"
    if not owner_only(config_path, limit=MAX_MEDIA_CONFIG_BYTES, exact_mode=0o600):
        findings.config("media_config")
        return {"config_present": False, "tool_sha256": {}}
    config_findings = Findings()
    config = read_json(config_path, MAX_MEDIA_CONFIG_BYTES, "media_config", config_findings)
    if config_findings.static:
        findings.config("media_config")
        return {"config_present": True, "tool_sha256": {}}
    expected_paths = {
        "ffmpeg_path": tools / "ffmpeg",
        "ffprobe_path": tools / "ffprobe",
        "staging_root": staging,
    }
    contract_valid = (
        set(config) == MEDIA_FIELDS
        and config.get("schema") == MEDIA_SCHEMA
        and config.get("output_profile") == MEDIA_PROFILE
        and all(
            type(config.get(field)) is int and 0 < config[field] <= maximum
            for field, maximum in MEDIA_LIMITS.items()
        )
        and config.get("max_total_output_bytes", 0)
        >= config.get("max_output_part_bytes", 1)
    )
    if not contract_valid:
        findings.config("media_config_contract")
    tool_hashes = {}
    for field, expected in expected_paths.items():
        try:
            canonical = expected.resolve(strict=True)
            binding_ok = config.get(field) == str(canonical)
            containment_ok = canonical == expected and canonical.is_relative_to(media)
        except (OSError, RuntimeError, ValueError):
            binding_ok = containment_ok = False
        if not binding_ok or not containment_ok:
            findings.config("media_stable_bindings")
    for name in ("ffmpeg", "ffprobe"):
        path = tools / name
        if not owner_only(
            path, limit=MAX_ARTIFACT_BYTES, executable=True, exact_mode=0o500
        ):
            findings.config(f"media_tool:{name}")
            continue
        value = sha256_file(path, f"media_tool:{name}", Findings(), executable=True)
        if value:
            tool_hashes[name] = value
    return {
        "config_present": True,
        "contract_valid": contract_valid,
        "schema": MEDIA_SCHEMA,
        "output_profile": MEDIA_PROFILE,
        "tool_sha256": tool_hashes,
    }


def audit_operator_config(data_root, role, findings):
    protected = data_root / "protected-content"
    requirements = (
        (protected / "chain-provider.json", "chain_config"),
        (protected / "custody-composition.json", "custody_composition"),
    )
    facts = {}
    for path, name in requirements:
        present = owner_only(path, limit=MAX_CONFIG_BYTES)
        facts[name] = present
        if not present:
            findings.config(name)
    if role == "custody-node":
        state = owner_only(protected / "custody-provider/inactive", directory=True)
        facts["custody_state"] = state
        if not state:
            findings.config("custody_state")
    return facts


def build_receipt(args):
    findings = Findings()
    if not ID_RE.fullmatch(args.platform):
        findings.artifact("platform")
    if not ID_RE.fullmatch(args.profile):
        findings.artifact("profile")
    source_root = canonical_input(args.source_root, "source_root", findings)
    data_root = canonical_input(args.installed_data_root, "installed_data_root", findings)
    runtime_path = canonical_input(args.installed_runtime, "installed_runtime", findings)
    base = {
        "schema": SCHEMA,
        "version": 1,
        "scope": "installed_static_only",
        "static_ok": False,
        "ready_for_active_proof": False,
        "role": args.role,
        "platform": args.platform,
        "profile": args.profile,
    }
    if not all((source_root, data_root, runtime_path)):
        return finish_receipt(base, findings)

    if disposable_install_location(data_root, source_root):
        findings.artifact("installed_data_location")
    if disposable_install_location(runtime_path, source_root):
        findings.artifact("installed_runtime_location")

    command_path = source_root / "scripts" / Path(__file__).name
    try:
        if command_path.resolve(strict=True) != Path(__file__).resolve(strict=True):
            findings.artifact("audit_command_binding")
    except (OSError, RuntimeError):
        findings.artifact("audit_command_binding")

    commit, tree, clean = git_identity(source_root, findings)
    source_manifest_path = source_root / "components.json"
    installed_manifest_path = data_root / "components.json"
    source_manifest = read_json(
        source_manifest_path, MAX_MANIFEST_BYTES, "source_components", findings
    )
    installed_manifest = read_json(
        installed_manifest_path, MAX_MANIFEST_BYTES, "installed_components", findings
    )
    source_manifest_sha = sha256_file(source_manifest_path, "source_components", findings)
    installed_manifest_sha = sha256_file(
        installed_manifest_path, "installed_components", findings
    )
    for manifest, name in (
        (source_manifest, "source_components"),
        (installed_manifest, "installed_components"),
    ):
        if manifest is not None and manifest.get("schema") != "elastos.components/v1":
            findings.artifact(name)

    selected = []
    metadata = {"source": {}, "installed": {}}
    artifacts = {}
    if installed_manifest is not None:
        profiles = installed_manifest.get("profiles")
        profile = profiles.get(args.profile) if isinstance(profiles, dict) else None
        components = profile.get("components") if isinstance(profile, dict) else None
        if (
            not isinstance(components, list)
            or not components
            or len(components) > MAX_PROFILE_COMPONENTS
            or len(set(components)) != len(components)
            or any(not isinstance(name, str) or not ID_RE.fullmatch(name) for name in components)
        ):
            findings.artifact("profile")
        else:
            selected = sorted(components)
            for name in ROLE_REQUIRED[args.role]:
                if name not in components:
                    findings.artifact(f"profile_component:{name}")
        metadata["installed"] = verify_canonical_metadata(installed_manifest, findings)
        audit_capsule_exposure(source_root, data_root, installed_manifest, findings)
        required = list(ROLE_REQUIRED[args.role])
        delegate_checks(source_root, data_root, args.platform, args.profile, required, findings)
        artifacts = audit_artifacts(
            source_root,
            data_root,
            runtime_path,
            installed_manifest,
            args.platform,
            required,
            findings,
        )
    if source_manifest is not None:
        metadata["source"] = verify_canonical_metadata(source_manifest, findings)
        audit_capsule_exposure(source_root, data_root, source_manifest, findings)

    media = audit_media(data_root, findings)
    operator = audit_operator_config(data_root, args.role, findings)
    canonical_selected = sorted(set(selected).intersection(CANONICAL))
    provisional_selected = sorted(set(selected).intersection(PROVISIONAL))
    if provisional_selected and canonical_selected:
        declared_mode = "pre_cutover_coexistence"
    elif provisional_selected:
        declared_mode = "provisional_selected"
    elif canonical_selected:
        declared_mode = "canonical_declared_unproven"
    else:
        declared_mode = "unconfigured"

    base.update(
        {
            "source": {
                "commit": commit,
                "tree": tree,
                "clean": clean,
                "components_sha256": source_manifest_sha,
            },
            "installed": {
                "components_sha256": installed_manifest_sha,
                "runtime_sha256": (artifacts.get("runtime") or {}).get("installed_sha256"),
            },
            "artifacts": artifacts,
            "canonical_installation": {
                "metadata": metadata,
                "required": sorted(ROLE_REQUIRED[args.role]),
                "installed_sha256": {
                    name: value
                    for name, value in (artifacts.get("installed_sha256") or {}).items()
                    if name in CANONICAL
                },
            },
            "active_path": {
                "status": "active_proof_pending",
                "declared_mode": declared_mode,
                "canonical_selected": canonical_selected,
                "provisional_selected": provisional_selected,
            },
            "operator_configuration": operator,
            "media": media,
        }
    )
    return finish_receipt(base, findings)


def finish_receipt(receipt, findings):
    receipt["static_ok"] = not findings.static
    receipt["ready_for_active_proof"] = not findings.static and not findings.operator
    receipt["findings"] = {
        "source_static_artifact_failures": sorted(findings.static),
        "operator_configuration_prerequisites": sorted(findings.operator),
        "active_installed_proof_prerequisites": list(ACTIVE_PROOF_PREREQUISITES),
    }
    return receipt


def bounded_receipt(receipt):
    encoded = json.dumps(receipt, separators=(",", ":"), sort_keys=True).encode("utf-8")
    if len(encoded) <= MAX_RECEIPT_BYTES:
        return receipt, encoded
    overflow_receipt = {
        "schema": SCHEMA,
        "version": 1,
        "scope": "installed_static_only",
        "static_ok": False,
        "ready_for_active_proof": False,
        "findings": {
            "source_static_artifact_failures": ["receipt_bounds"],
            "operator_configuration_prerequisites": [],
            "active_installed_proof_prerequisites": list(ACTIVE_PROOF_PREREQUISITES),
        },
    }
    encoded = json.dumps(
        overflow_receipt, separators=(",", ":"), sort_keys=True
    ).encode("utf-8")
    return overflow_receipt, encoded


def main(argv):
    args = parse_args(argv)
    try:
        receipt = build_receipt(args)
    except Exception:
        findings = Findings()
        findings.artifact("internal_audit")
        receipt = finish_receipt(
            {"schema": SCHEMA, "version": 1, "scope": "installed_static_only"},
            findings,
        )
    receipt, encoded = bounded_receipt(receipt)
    sys.stdout.buffer.write(encoded + b"\n")
    failures = receipt.get("findings", {}).get("source_static_artifact_failures", [])
    prerequisites = receipt.get("findings", {}).get("operator_configuration_prerequisites", [])
    if failures or prerequisites:
        names = sorted(set(failures).union(prerequisites))
        print("static audit findings: " + ", ".join(names), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
