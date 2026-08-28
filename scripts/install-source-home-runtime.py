#!/usr/bin/env python3
import argparse
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
from datetime import datetime, timezone
from pathlib import Path


SCHEMA = "elastos.source-home.installation-receipt/v1"
RECEIPT_NAME = "source-home-installation.json"
CAPSULE_RECEIPT_NAME = "source-home-capsules.json"
MAX_RUNTIME_BYTES = 2 * 1024 * 1024 * 1024
MAX_COMPONENTS_BYTES = 4 * 1024 * 1024
MAX_CAPSULE_RECEIPT_BYTES = 4 * 1024 * 1024
MAX_RECEIPT_BYTES = 16 * 1024
GIT_ID_RE = re.compile(r"^[0-9a-f]{40,64}$")
PLATFORM_RE = re.compile(r"^[a-z0-9][a-z0-9-]{0,31}$")


class InstallError(Exception):
    pass


def fail(reason):
    raise InstallError(reason)


def parse_args(argv):
    parser = argparse.ArgumentParser(
        description="Install the source-home Runtime and publish its artifact receipt."
    )
    parser.add_argument("--source-root", required=True, type=Path)
    parser.add_argument("--data-dir", required=True, type=Path)
    parser.add_argument("--built-runtime", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    return parser.parse_args(argv)


def require_directory(path, *, exact_mode=None):
    try:
        metadata = path.lstat()
    except OSError:
        fail("required_directory")
    if (
        not stat.S_ISDIR(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_mode & 0o022
        or (exact_mode is not None and stat.S_IMODE(metadata.st_mode) != exact_mode)
    ):
        fail("unsafe_directory")


def ensure_receipts_directory(data_dir):
    receipts = data_dir / "receipts"
    try:
        os.mkdir(receipts, 0o700)
    except FileExistsError:
        require_directory(receipts)
        os.chmod(receipts, 0o700, follow_symlinks=False)
    except OSError:
        fail("receipt_directory")
    require_directory(receipts, exact_mode=0o700)
    return receipts


def require_safe_destination(path, mode):
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return
    except OSError:
        fail("destination_metadata")
    if (
        not stat.S_ISREG(metadata.st_mode)
        or stat.S_ISLNK(metadata.st_mode)
        or metadata.st_uid != os.geteuid()
        or metadata.st_nlink != 1
        or stat.S_IMODE(metadata.st_mode) != mode
    ):
        fail("unsafe_destination")


def open_regular(path, limit, *, executable=False, trusted_runtime=False):
    flags = os.O_RDONLY
    if hasattr(os, "O_NOFOLLOW"):
        flags |= os.O_NOFOLLOW
    try:
        descriptor = os.open(path, flags)
        metadata = os.fstat(descriptor)
        if (
            not stat.S_ISREG(metadata.st_mode)
            or metadata.st_nlink != 1
            or metadata.st_size <= 0
            or metadata.st_size > limit
            or (executable and metadata.st_mode & 0o100 == 0)
            or (
                trusted_runtime
                and (
                    metadata.st_uid not in {0, os.geteuid()}
                    or metadata.st_mode & 0o022
                )
            )
        ):
            raise OSError
        return descriptor, metadata
    except OSError:
        try:
            os.close(descriptor)
        except (NameError, OSError):
            pass
        fail("unsafe_artifact")


def hash_descriptor(descriptor, metadata, limit):
    digest = hashlib.sha256()
    total = 0
    while True:
        chunk = os.read(descriptor, min(1024 * 1024, limit + 1 - total))
        if not chunk:
            break
        total += len(chunk)
        if total > limit:
            fail("artifact_oversize")
        digest.update(chunk)
    final = os.fstat(descriptor)
    if (
        total != metadata.st_size
        or final.st_size != metadata.st_size
        or final.st_mtime_ns != metadata.st_mtime_ns
    ):
        fail("artifact_changed")
    return "sha256:" + digest.hexdigest()


def hash_file(path, limit, *, executable=False, trusted_runtime=False):
    descriptor, metadata = open_regular(
        path, limit, executable=executable, trusted_runtime=trusted_runtime
    )
    try:
        return hash_descriptor(descriptor, metadata, limit)
    finally:
        os.close(descriptor)


def git_value(source_root, revision):
    env = os.environ.copy()
    env["GIT_OPTIONAL_LOCKS"] = "0"
    try:
        result = subprocess.run(
            ["git", "-C", str(source_root), "rev-parse", "--verify", revision],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        fail("source_git_identity")
    value = result.stdout.decode("ascii", errors="ignore").strip().lower()
    if result.returncode != 0 or not GIT_ID_RE.fullmatch(value):
        fail("source_git_identity")
    return value


def source_identity(source_root):
    env = os.environ.copy()
    env["GIT_OPTIONAL_LOCKS"] = "0"
    try:
        status = subprocess.run(
            [
                "git",
                "-C",
                str(source_root),
                "status",
                "--porcelain=v1",
                "--untracked-files=all",
            ],
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            timeout=15,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        fail("source_git_status")
    if status.returncode != 0 or len(status.stdout) > 1024 * 1024:
        fail("source_git_status")
    return {
        "commit": git_value(source_root, "HEAD"),
        "tree": git_value(source_root, "HEAD^{tree}"),
        "clean": not status.stdout,
    }


def staged_file(parent, mode):
    try:
        descriptor, name = tempfile.mkstemp(prefix=".source-home-install.", dir=parent)
        os.fchmod(descriptor, mode)
        return descriptor, Path(name)
    except OSError:
        fail("stage_create")


def stage_runtime(source, destination_parent):
    source_descriptor, source_metadata = open_regular(
        source, MAX_RUNTIME_BYTES, executable=True, trusted_runtime=True
    )
    staged_descriptor = None
    staged_path = None
    try:
        staged_descriptor, staged_path = staged_file(destination_parent, 0o700)
        digest = hashlib.sha256()
        total = 0
        while True:
            chunk = os.read(source_descriptor, 1024 * 1024)
            if not chunk:
                break
            total += len(chunk)
            if total > MAX_RUNTIME_BYTES:
                fail("runtime_oversize")
            digest.update(chunk)
            view = memoryview(chunk)
            while view:
                written = os.write(staged_descriptor, view)
                view = view[written:]
        final_source = os.fstat(source_descriptor)
        if (
            total != source_metadata.st_size
            or final_source.st_size != source_metadata.st_size
            or final_source.st_mtime_ns != source_metadata.st_mtime_ns
        ):
            fail("runtime_changed")
        os.fsync(staged_descriptor)
        os.close(staged_descriptor)
        staged_descriptor = None
        value = "sha256:" + digest.hexdigest()
        if hash_file(staged_path, MAX_RUNTIME_BYTES, executable=True) != value:
            fail("staged_runtime_mismatch")
        return staged_path, value
    except Exception:
        if staged_descriptor is not None:
            os.close(staged_descriptor)
        if staged_path is not None:
            staged_path.unlink(missing_ok=True)
        raise
    finally:
        os.close(source_descriptor)


def stage_receipt(parent, receipt):
    payload = (json.dumps(receipt, sort_keys=True, indent=2) + "\n").encode("utf-8")
    if len(payload) > MAX_RECEIPT_BYTES:
        fail("receipt_oversize")
    descriptor = None
    staged_path = None
    try:
        descriptor, staged_path = staged_file(parent, 0o600)
        view = memoryview(payload)
        while view:
            written = os.write(descriptor, view)
            view = view[written:]
        os.fsync(descriptor)
        os.close(descriptor)
        descriptor = None
        return staged_path
    except Exception:
        if descriptor is not None:
            os.close(descriptor)
        if staged_path is not None:
            staged_path.unlink(missing_ok=True)
        raise


def fsync_directory(path):
    try:
        descriptor = os.open(path, os.O_RDONLY)
        try:
            os.fsync(descriptor)
        finally:
            os.close(descriptor)
    except OSError:
        fail("directory_sync")


def install(
    args,
    runtime_stager=stage_runtime,
    runtime_publisher=os.replace,
    post_runtime_hook=None,
):
    if not PLATFORM_RE.fullmatch(args.platform):
        fail("platform")
    try:
        source_root = args.source_root.resolve(strict=True)
        data_dir = args.data_dir.absolute()
        built_runtime = args.built_runtime.absolute()
    except (OSError, RuntimeError):
        fail("input_path")
    require_directory(source_root)
    require_directory(data_dir, exact_mode=0o700)
    bin_dir = data_dir / "bin"
    require_directory(bin_dir)
    receipts_dir = ensure_receipts_directory(data_dir)

    installed_runtime = bin_dir / "elastos"
    receipt_path = receipts_dir / RECEIPT_NAME
    require_safe_destination(installed_runtime, 0o700)
    require_safe_destination(receipt_path, 0o600)

    components_hash = hash_file(data_dir / "components.json", MAX_COMPONENTS_BYTES)
    capsules_hash = hash_file(
        receipts_dir / CAPSULE_RECEIPT_NAME, MAX_CAPSULE_RECEIPT_BYTES
    )
    source = source_identity(source_root)
    built_hash = hash_file(
        built_runtime, MAX_RUNTIME_BYTES, executable=True, trusted_runtime=True
    )
    runtime_stage = receipt_stage = invalidated_receipt = None
    runtime_replaced = False
    receipt_published = False
    completed = False
    try:
        runtime_stage, staged_hash = runtime_stager(built_runtime, bin_dir)
        if staged_hash != built_hash:
            fail("built_runtime_mismatch")
        receipt = {
            "schema": SCHEMA,
            "source": source,
            "runtime": {
                "built_sha256": built_hash,
                "installed_sha256": staged_hash,
                "parity": True,
            },
            "components_sha256": components_hash,
            "source_home_capsule_metadata_receipt_sha256": capsules_hash,
            "platform": args.platform,
            "installation_time_utc": datetime.now(timezone.utc)
            .replace(microsecond=0)
            .isoformat()
            .replace("+00:00", "Z"),
        }
        receipt_stage = stage_receipt(receipts_dir, receipt)

        if receipt_path.exists():
            descriptor, invalidation_stage = staged_file(receipts_dir, 0o600)
            os.close(descriptor)
            try:
                os.replace(receipt_path, invalidation_stage)
            except OSError:
                invalidation_stage.unlink(missing_ok=True)
                fail("receipt_invalidation")
            invalidated_receipt = invalidation_stage
            fsync_directory(receipts_dir)

        try:
            runtime_publisher(runtime_stage, installed_runtime)
        except OSError:
            fail("runtime_publication")
        runtime_stage = None
        runtime_replaced = True
        os.chmod(installed_runtime, 0o700, follow_symlinks=False)
        fsync_directory(bin_dir)
        if (
            hash_file(installed_runtime, MAX_RUNTIME_BYTES, executable=True)
            != built_hash
        ):
            fail("installed_runtime_mismatch")

        if invalidated_receipt is not None:
            invalidated_receipt.unlink()
            invalidated_receipt = None
            fsync_directory(receipts_dir)
        if post_runtime_hook is not None:
            post_runtime_hook()
        try:
            os.replace(receipt_stage, receipt_path)
        except OSError:
            fail("receipt_publication")
        receipt_stage = None
        receipt_published = True
        os.chmod(receipt_path, 0o600, follow_symlinks=False)
        fsync_directory(receipts_dir)
        require_safe_destination(installed_runtime, 0o700)
        require_safe_destination(receipt_path, 0o600)
        if receipt_path.stat().st_size > MAX_RECEIPT_BYTES:
            fail("receipt_oversize")
        completed = True
        return receipt
    finally:
        if invalidated_receipt is not None:
            if runtime_replaced:
                invalidated_receipt.unlink(missing_ok=True)
            else:
                try:
                    os.replace(invalidated_receipt, receipt_path)
                except OSError:
                    pass
            try:
                fsync_directory(receipts_dir)
            except InstallError:
                pass
        if receipt_published and not completed:
            receipt_path.unlink(missing_ok=True)
            try:
                fsync_directory(receipts_dir)
            except InstallError:
                pass
        if runtime_stage is not None:
            runtime_stage.unlink(missing_ok=True)
        if receipt_stage is not None:
            receipt_stage.unlink(missing_ok=True)


def main(argv=None):
    try:
        install(parse_args(sys.argv[1:] if argv is None else argv))
    except InstallError as error:
        print(
            f"source-home Runtime installation failed: {error}",
            file=sys.stderr,
        )
        return 1
    except Exception:
        print("source-home Runtime installation failed: internal_error", file=sys.stderr)
        return 1
    print("[source-home] installed stable Runtime and installation receipt")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
