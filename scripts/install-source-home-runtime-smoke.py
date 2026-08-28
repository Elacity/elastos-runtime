#!/usr/bin/env python3
import hashlib
import importlib.util
import json
import os
import re
import shutil
import stat
import subprocess
import tempfile
from pathlib import Path
from types import SimpleNamespace


ROOT = Path(__file__).resolve().parents[1]
INSTALLER = ROOT / "scripts" / "install-source-home-runtime.py"
SETUP = ROOT / "scripts" / "setup-source-home.sh"
SCHEMA = "elastos.source-home.installation-receipt/v1"
RECEIPT = Path("receipts/source-home-installation.json")
CAPSULE_RECEIPT = Path("receipts/source-home-capsules.json")
SHA256_RE = re.compile(r"^sha256:[0-9a-f]{64}$")


def load_installer():
    spec = importlib.util.spec_from_file_location("source_home_installer", INSTALLER)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def run(command, cwd=None, check=True):
    result = subprocess.run(
        command,
        cwd=cwd,
        stdin=subprocess.DEVNULL,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
        text=True,
    )
    if check and result.returncode != 0:
        raise AssertionError(
            f"command failed ({result.returncode}): {' '.join(command)}\n"
            f"stdout: {result.stdout}\nstderr: {result.stderr}"
        )
    return result


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def write(path, value, mode=0o600):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(value)
    path.chmod(mode)


def initialize_source(root):
    root.mkdir(mode=0o700)
    write(root / "tracked.txt", b"source\n", 0o600)
    run(["git", "init", "-q"], cwd=root)
    run(["git", "add", "tracked.txt"], cwd=root)
    run(
        [
            "git",
            "-c",
            "user.name=Source Home Smoke",
            "-c",
            "user.email=source-home-smoke@example.invalid",
            "commit",
            "-q",
            "-m",
            "fixture",
        ],
        cwd=root,
    )


def initialize_data(root):
    root.mkdir(mode=0o700)
    (root / "bin").mkdir(mode=0o700)
    (root / "receipts").mkdir(mode=0o700)
    write(root / "components.json", b'{"profiles":{"source-home":{}}}\n')
    write(
        root / CAPSULE_RECEIPT,
        b'{"schema":"elastos.source-home.managed-capsules/v1","capsules":[]}\n',
    )


def installer_command(source, data, built):
    return [
        "python3",
        str(INSTALLER),
        "--source-root",
        str(source),
        "--data-dir",
        str(data),
        "--built-runtime",
        str(built),
        "--platform",
        "linux-amd64",
    ]


def invoke(source, data, built, check=True):
    return run(installer_command(source, data, built), check=check)


def git_value(source, revision):
    return run(["git", "rev-parse", revision], cwd=source).stdout.strip()


def snapshot(root):
    values = {}
    for path in sorted(root.rglob("*")):
        if path.is_symlink():
            values[path.relative_to(root).as_posix()] = ("link", os.readlink(path))
        elif path.is_file():
            metadata = path.stat()
            values[path.relative_to(root).as_posix()] = (
                "file",
                sha256(path),
                stat.S_IMODE(metadata.st_mode),
                metadata.st_ino,
            )
        elif path.is_dir():
            metadata = path.stat()
            values[path.relative_to(root).as_posix()] = (
                "dir",
                stat.S_IMODE(metadata.st_mode),
            )
    return values


def assert_receipt(source, data, built, clean):
    receipt_path = data / RECEIPT
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if set(receipt) != {
        "schema",
        "source",
        "runtime",
        "components_sha256",
        "source_home_capsule_metadata_receipt_sha256",
        "platform",
        "installation_time_utc",
    }:
        raise AssertionError(f"unexpected receipt fields: {sorted(receipt)}")
    if receipt["schema"] != SCHEMA or receipt["platform"] != "linux-amd64":
        raise AssertionError("receipt schema or platform mismatch")
    if receipt["source"] != {
        "commit": git_value(source, "HEAD"),
        "tree": git_value(source, "HEAD^{tree}"),
        "clean": clean,
    }:
        raise AssertionError("source identity mismatch")
    built_hash = sha256(built)
    installed_hash = sha256(data / "bin/elastos")
    if receipt["runtime"] != {
        "built_sha256": built_hash,
        "installed_sha256": installed_hash,
        "parity": True,
    }:
        raise AssertionError("Runtime hash parity mismatch")
    if built_hash != installed_hash:
        raise AssertionError("installed Runtime differs from the built Runtime")
    if receipt["components_sha256"] != sha256(data / "components.json"):
        raise AssertionError("components.json hash mismatch")
    if receipt["source_home_capsule_metadata_receipt_sha256"] != sha256(
        data / CAPSULE_RECEIPT
    ):
        raise AssertionError("capsule metadata receipt hash mismatch")
    for value in (
        receipt["runtime"]["built_sha256"],
        receipt["runtime"]["installed_sha256"],
        receipt["components_sha256"],
        receipt["source_home_capsule_metadata_receipt_sha256"],
    ):
        if not SHA256_RE.fullmatch(value):
            raise AssertionError("receipt contains an invalid SHA-256 value")
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}Z", receipt["installation_time_utc"]):
        raise AssertionError("receipt installation time is not bounded UTC")
    payload = receipt_path.read_bytes()
    if len(payload) > 16 * 1024:
        raise AssertionError("receipt exceeds its size bound")
    for private_value in (str(source), str(data), str(built), str(Path.home())):
        if private_value.encode() in payload:
            raise AssertionError("receipt leaks a private absolute path")
    if stat.S_IMODE((data / "bin/elastos").stat().st_mode) != 0o700:
        raise AssertionError("installed Runtime is not owner-only executable")
    if stat.S_IMODE(receipt_path.stat().st_mode) != 0o600:
        raise AssertionError("installation receipt is not owner-only")
    if stat.S_IMODE(receipt_path.parent.stat().st_mode) != 0o700:
        raise AssertionError("installation receipt directory is not owner-only")
    if receipt_path.stat().st_nlink != 1 or (data / "bin/elastos").stat().st_nlink != 1:
        raise AssertionError("installed artifacts must have one link")


def assert_failed(source, data, built, reason):
    result = invoke(source, data, built, check=False)
    if result.returncode == 0 or reason not in result.stderr:
        raise AssertionError(
            f"expected failure {reason!r}; stdout={result.stdout!r} stderr={result.stderr!r}"
        )
    combined = (result.stdout + result.stderr).encode()
    for private_value in (str(source), str(data), str(built), str(Path.home())):
        if private_value.encode() in combined:
            raise AssertionError("failure output leaks a private absolute path")


def test_good_dirty_and_rerun(temp_root):
    source = temp_root / "source"
    data = temp_root / "installed"
    built = temp_root / "build/elastos"
    initialize_source(source)
    initialize_data(data)
    write(built, b"runtime-v1\n", 0o700)

    result = invoke(source, data, built)
    if result.stdout.strip() != "[source-home] installed stable Runtime and installation receipt":
        raise AssertionError("unexpected installer output")
    assert_receipt(source, data, built, clean=True)

    before = snapshot(data)
    runtime_inode = (data / "bin/elastos").stat().st_ino
    receipt_inode = (data / RECEIPT).stat().st_ino
    invoke(source, data, built)
    after = snapshot(data)
    if set(before) != set(after):
        raise AssertionError("exact rerun left an extra artifact")
    for name in before:
        if name not in {"bin/elastos", RECEIPT.as_posix()} and before[name] != after[name]:
            raise AssertionError(f"exact rerun changed unrelated state: {name}")
    if runtime_inode == (data / "bin/elastos").stat().st_ino:
        raise AssertionError("exact rerun did not atomically replace the Runtime")
    if receipt_inode == (data / RECEIPT).stat().st_ino:
        raise AssertionError("exact rerun did not atomically replace the receipt")
    assert_receipt(source, data, built, clean=True)

    (source / "tracked.txt").write_text("dirty source\n", encoding="utf-8")
    invoke(source, data, built)
    assert_receipt(source, data, built, clean=False)
    if any(path.name.startswith(".source-home-install.") for path in data.rglob("*")):
        raise AssertionError("installer left a staged file")


def test_installed_mismatch(temp_root):
    installer = load_installer()
    source = temp_root / "mismatch-source"
    data = temp_root / "mismatch-installed"
    built = temp_root / "mismatch-build/elastos"
    initialize_source(source)
    initialize_data(data)
    write(built, b"expected-runtime\n", 0o700)

    def corrupt_stage(runtime, parent):
        staged, expected_hash = installer.stage_runtime(runtime, parent)
        staged.write_bytes(b"corrupt-runtime\n")
        staged.chmod(0o700)
        return staged, expected_hash

    args = SimpleNamespace(
        source_root=source,
        data_dir=data,
        built_runtime=built,
        platform="linux-amd64",
    )
    try:
        installer.install(args, runtime_stager=corrupt_stage)
    except installer.InstallError as error:
        if str(error) != "installed_runtime_mismatch":
            raise AssertionError(f"unexpected mismatch failure: {error}") from error
    else:
        raise AssertionError("installer accepted mismatched installed Runtime bytes")
    if (data / RECEIPT).exists():
        raise AssertionError("Runtime parity failure published a success receipt")
    if any(path.name.startswith(".source-home-install.") for path in data.rglob("*")):
        raise AssertionError("mismatch failure left a staged file")


def test_post_runtime_receipt_failure(temp_root):
    installer = load_installer()
    source = temp_root / "publication-source"
    data = temp_root / "publication-installed"
    built = temp_root / "publication-build/elastos"
    initialize_source(source)
    initialize_data(data)
    write(built, b"runtime-v1\n", 0o700)
    invoke(source, data, built)
    old_receipt = (data / RECEIPT).read_bytes()

    write(built, b"runtime-v2\n", 0o700)

    def fail_after_runtime():
        raise installer.InstallError("injected_post_runtime_failure")

    args = SimpleNamespace(
        source_root=source,
        data_dir=data,
        built_runtime=built,
        platform="linux-amd64",
    )
    try:
        installer.install(args, post_runtime_hook=fail_after_runtime)
    except installer.InstallError as error:
        if str(error) != "injected_post_runtime_failure":
            raise AssertionError(f"unexpected publication failure: {error}") from error
    else:
        raise AssertionError("post-Runtime fault did not stop receipt publication")
    if (data / RECEIPT).exists():
        if (data / RECEIPT).read_bytes() == old_receipt:
            raise AssertionError("new Runtime retained the stale success receipt")
        raise AssertionError("post-Runtime fault published a receipt")
    if (data / "bin/elastos").read_bytes() != b"runtime-v2\n":
        raise AssertionError("fault injection did not reach the post-Runtime boundary")
    if any(path.name.startswith(".source-home-install.") for path in data.rglob("*")):
        raise AssertionError("post-Runtime failure left an invalidated receipt or stage")


def test_runtime_publication_restores_pair(temp_root):
    installer = load_installer()
    source = temp_root / "restore-source"
    data = temp_root / "restore-installed"
    built = temp_root / "restore-build/elastos"
    initialize_source(source)
    initialize_data(data)
    write(built, b"runtime-v1\n", 0o700)
    invoke(source, data, built)
    old_runtime = (data / "bin/elastos").read_bytes()
    old_receipt = (data / RECEIPT).read_bytes()

    write(built, b"runtime-v2\n", 0o700)

    def fail_runtime_publication(_source, _destination):
        raise OSError("injected publication failure")

    args = SimpleNamespace(
        source_root=source,
        data_dir=data,
        built_runtime=built,
        platform="linux-amd64",
    )
    try:
        installer.install(args, runtime_publisher=fail_runtime_publication)
    except installer.InstallError as error:
        if str(error) != "runtime_publication":
            raise AssertionError(f"unexpected Runtime publication failure: {error}") from error
    else:
        raise AssertionError("injected Runtime publication failure was accepted")
    if (data / "bin/elastos").read_bytes() != old_runtime:
        raise AssertionError("Runtime publication failure changed the old Runtime")
    if (data / RECEIPT).read_bytes() != old_receipt:
        raise AssertionError("Runtime publication failure did not restore the exact receipt")
    if any(path.name.startswith(".source-home-install.") for path in data.rglob("*")):
        raise AssertionError("Runtime publication failure left a staged file")


def test_unsafe_destinations(temp_root):
    source = temp_root / "unsafe-source"
    built = temp_root / "unsafe-build/elastos"
    initialize_source(source)
    write(built, b"runtime\n", 0o700)

    data = temp_root / "runtime-link-installed"
    initialize_data(data)
    target = temp_root / "outside-runtime"
    write(target, b"preserve\n", 0o600)
    (data / "bin/elastos").symlink_to(target)
    assert_failed(source, data, built, "unsafe_destination")
    if target.read_bytes() != b"preserve\n":
        raise AssertionError("Runtime symlink rejection changed its target")

    data = temp_root / "receipt-link-installed"
    initialize_data(data)
    target = temp_root / "outside-receipt"
    write(target, b"preserve\n", 0o600)
    (data / RECEIPT).symlink_to(target)
    assert_failed(source, data, built, "unsafe_destination")
    if target.read_bytes() != b"preserve\n":
        raise AssertionError("receipt symlink rejection changed its target")

    data = temp_root / "unsafe-parent-installed"
    initialize_data(data)
    (data / "receipts").chmod(0o777)
    assert_failed(source, data, built, "unsafe_directory")
    if (data / RECEIPT).exists() or (data / "bin/elastos").exists():
        raise AssertionError("unsafe parent failure published an artifact")

    data = temp_root / "unsafe-file-installed"
    initialize_data(data)
    write(data / "bin/elastos", b"old\n", 0o755)
    assert_failed(source, data, built, "unsafe_destination")


def test_untrusted_built_runtime(temp_root):
    source = temp_root / "untrusted-source"
    data = temp_root / "untrusted-installed"
    built = temp_root / "untrusted-build/elastos"
    initialize_source(source)
    initialize_data(data)
    write(built, b"runtime\n", 0o770)
    assert_failed(source, data, built, "unsafe_artifact")
    if (data / RECEIPT).exists() or (data / "bin/elastos").exists():
        raise AssertionError("untrusted built Runtime published an installed artifact")


def assert_setup_orchestration():
    setup = SETUP.read_text(encoding="utf-8")
    call = 'python3 "${ROOT}/scripts/install-source-home-runtime.py"'
    if setup.count(call) != 1:
        raise AssertionError("setup must have one stable Runtime receipt publisher")
    main = setup.split('echo "[setup-source-home] repo:', 1)[1]
    if not (
        main.index("install_collaboration_startup_config\n")
        < main.index(call)
        < main.index("[setup-source-home] artifacts installed")
    ):
        raise AssertionError("stable Runtime receipt must be the final setup publication")
    config_exit = main.index('echo "[setup-source-home] config-only artifacts installed"')
    if config_exit >= main.index(call):
        raise AssertionError("config-only mode reaches stable Runtime publication")
    for binding in (
        '--source-root "${ROOT}"',
        '--data-dir "${DATA_DIR}"',
        'release elastos)',
        '--platform "${PLATFORM}"',
    ):
        if binding not in main:
            raise AssertionError(f"setup stable Runtime binding missing: {binding}")


def main():
    assert_setup_orchestration()
    outer = Path(tempfile.mkdtemp(prefix="source-home-runtime-smoke."))
    try:
        fixture = outer / "fixture"
        fixture.mkdir(mode=0o700)
        test_good_dirty_and_rerun(fixture)
        test_installed_mismatch(fixture)
        test_post_runtime_receipt_failure(fixture)
        test_runtime_publication_restores_pair(fixture)
        test_unsafe_destinations(fixture)
        test_untrusted_built_runtime(fixture)
        unexpected = [path for path in outer.iterdir() if path.name != "fixture"]
        if unexpected:
            raise AssertionError(f"installer left residue outside the fixture: {unexpected}")
    finally:
        shutil.rmtree(outer)
    print("PASS source-home Runtime installation receipt smoke")


if __name__ == "__main__":
    main()
