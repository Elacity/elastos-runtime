#!/usr/bin/env python3
"""Package an existing verified image set for the Runtime component installer.

The release publisher supplies the artifact path and merges the emitted external
component row into its signed components manifest. This script does not publish.
"""
import argparse
import gzip
import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            result.update(chunk)
    return result.hexdigest()


def package(image, platform, archive, manifest_output, release_path):
    if platform not in {"darwin-arm64", "linux-arm64", "linux-amd64"}:
        raise ValueError(f"unsupported Browser image platform: {platform}")
    if (not release_path.endswith(".tar.gz") or release_path.startswith("/")
            or any(part in {"", ".", ".."} for part in release_path.split("/"))):
        raise ValueError("release path must be a relative tar.gz artifact path")
    image, archive, manifest_output = (Path(p).resolve() for p in (image, archive, manifest_output))
    if archive == manifest_output or any(p == image or image in p.parents for p in (archive, manifest_output)):
        raise ValueError("package outputs must be distinct and outside the source image set")
    files = {name: image / name for name in ("rootfs.ext4", "vmlinux", "initrd", "browser-vm-rootfs-manifest.json")}
    if any(not p.is_file() or p.is_symlink() for p in files.values()):
        raise ValueError("image package requires four regular source files")
    env = {k: v for k, v in os.environ.items() if not k.startswith("ELASTOS_BROWSER_VM_")}
    env.update(ELASTOS_BROWSER_VM_PLATFORM=platform, ELASTOS_BROWSER_VM_DATA_DIR=str(image.parent),
               ELASTOS_BROWSER_VM_ROOTFS=str(files["rootfs.ext4"]),
               ELASTOS_BROWSER_VM_ROOTFS_MANIFEST=str(files["browser-vm-rootfs-manifest.json"]),
               ELASTOS_BROWSER_VM_KERNEL=str(files["vmlinux"]))
    env["ELASTOS_BROWSER_VM_INITRAMFS" if platform == "darwin-arm64" else "ELASTOS_BROWSER_VM_INITRD"] = str(files["initrd"])
    verified = subprocess.run([str(Path(__file__).resolve().parent / "browser-vm-artifact-preflight.sh"),
                               "--verify-image-set"], env=env, capture_output=True, text=True, timeout=600)
    proof = json.loads(verified.stdout)
    if verified.returncode or proof.get("schema") != "elastos.browser.vm-image-set/v1" or proof.get("ok") is not True:
        raise ValueError("Browser image set failed verification; source and existing package preserved")
    receipt = json.loads(files["browser-vm-rootfs-manifest.json"].read_bytes())
    # Publish the byte/contract receipt. Operator paths remain in the build receipt.
    portable = {k: receipt[k] for k in ("schema", "ok", "target_platform", "size", "sha256")}
    for name in ("kernel", "initrd"):
        portable[name] = {k: receipt[name][k] for k in ("size", "sha256")}
    preflight = receipt["preflight"]
    portable["preflight"] = {k: preflight[k] for k in ("ok", "audio_default_ready", "manifest", "missing", "manifest_errors", "script_errors")}
    for group in ("required", "optional_audio"):
        portable["preflight"][group] = {name: {"ok": entry["ok"]} for name, entry in preflight[group].items()}
    receipt_bytes = (json.dumps(portable, sort_keys=True, indent=2) + "\n").encode()
    archive.parent.mkdir(parents=True, exist_ok=True)
    manifest_output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".browser-image-package-", dir=archive.parent) as temp:
        staged = Path(temp) / "image.tar.gz"
        with staged.open("wb") as raw, gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.GNU_FORMAT) as tar:
                for name, source in sorted(files.items()):
                    content = io.BytesIO(receipt_bytes) if name == "browser-vm-rootfs-manifest.json" else source.open("rb")
                    with content:
                        size = len(receipt_bytes) if name == "browser-vm-rootfs-manifest.json" else source.stat().st_size
                        info = tarfile.TarInfo("browser-vm-image/" + name)
                        info.mode, info.size, info.mtime = 0o644, size, 0
                        tar.addfile(info, content)
        # Recheck source hashes after reading: a concurrent build cannot silently
        # turn the archive into a mixed image set.
        for name, entry in (("rootfs.ext4", receipt), ("vmlinux", receipt["kernel"]), ("initrd", receipt["initrd"])):
            if digest(files[name]) != entry["sha256"]:
                raise ValueError("source image changed during packaging")
        checksum, size = "sha256:" + digest(staged), staged.stat().st_size
        if size > 200 * 1024 * 1024:
            raise ValueError("image archive exceeds the current 200 MiB Carrier file limit; release acquisition requires a supported artifact transport")
        component = {"install_path": "browser-vm/image-set", "description": "Verified Browser local Engine image dependency",
                     "platforms": {platform: {"install_path": "browser-vm/image-set", "extract_path": "browser-vm-image",
                                              "strategy": "browser-vm-image", "release_path": release_path,
                                              "checksum": checksum, "size": size}}}
        overlay = {"external": {"browser-vm-image": component}, "profiles": {}}
        metadata_bytes = (json.dumps(overlay, sort_keys=True, indent=2) + "\n").encode()
        # Release outputs are immutable. An identical rerun succeeds without
        # replacing either file; a new release uses a new pair of output paths.
        if archive.exists() or manifest_output.exists():
            if (archive.is_file() and manifest_output.is_file()
                    and digest(archive) == checksum.removeprefix("sha256:")
                    and manifest_output.read_bytes() == metadata_bytes):
                return overlay
            raise ValueError("package outputs already exist; use new immutable archive and manifest output paths")
        with tempfile.TemporaryDirectory(prefix=".browser-image-metadata-", dir=manifest_output.parent) as metadata_temp:
            pending = Path(metadata_temp) / "components.json"
            pending.write_bytes(metadata_bytes)
            # Same-filesystem hard links publish complete files without
            # overwriting a concurrent writer. Metadata failure removes only
            # this invocation's newly created archive.
            os.link(staged, archive)
            try:
                os.link(pending, manifest_output)
            except OSError:
                if archive.exists() and archive.samefile(staged):
                    archive.unlink()
                raise
    return overlay


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--image-dir", required=True, type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--archive", required=True, type=Path)
    parser.add_argument("--manifest-output", required=True, type=Path)
    parser.add_argument("--release-path", required=True)
    args = parser.parse_args()
    try:
        package(args.image_dir, args.platform, args.archive, args.manifest_output, args.release_path)
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as exc:
        parser.exit(1, f"Browser image packaging failed: {exc}\n")


if __name__ == "__main__":
    main()
