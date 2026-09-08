#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
exec python3 - "$repo_root" "$@" <<'PY'
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys

repo_root = Path(sys.argv.pop(1))
parser = argparse.ArgumentParser(description="Link one verified Browser VM image set; preserve the source artifacts.")
parser.add_argument("--data-dir", type=Path, required=True)
parser.add_argument("--platform", choices=["darwin-arm64", "linux-arm64", "linux-amd64"], required=True)
parser.add_argument("--artifact-data-dir", type=Path,
                    default=os.environ.get("ELASTOS_BROWSER_VM_ARTIFACT_DATA_DIR"))
args = parser.parse_args()
for value in (args.data_dir, args.artifact_data_dir):
    if value is not None and not value.is_absolute():
        parser.error("artifact and data directories must be absolute")
data = args.data_dir
candidates = []
if args.artifact_data_dir is not None:
    candidates.append(args.artifact_data_dir)
elif "managed-runtimes" in data.parts:
    candidates.append(Path(*data.parts[:data.parts.index("managed-runtimes")]))

rootfs_rel = "browser-vm/rootfs.ext4"
manifest_rel = "browser-vm/browser-vm-rootfs-manifest.json"
initrd_rel = "bin/initrd" if args.platform == "darwin-arm64" else "browser-vm/initrd"
result = {"schema": "elastos.setup-source-home.browser-artifacts/v1", "ok": True,
          "platform": args.platform, "data_dir": str(data),
          "candidate_dirs": [str(p) for p in candidates], "linked": 0,
          "skipped_existing": 0, "missing": 0, "image_set_verified": False}


def finish(error=None):
    if error:
        result.update(ok=False, error=error)
    print(json.dumps(result, separators=(",", ":")))
    sys.exit(1 if error else 0)


# A receipt belongs to the rootfs that selected the set. Kernel and initrd may
# use the build layout or installed layout, but every byte must match that receipt.
rootfs = data / rootfs_rel
if rootfs.exists():
    rootfs = rootfs.resolve()
else:
    rootfs = next((p / rootfs_rel for p in candidates if (p / rootfs_rel).is_file()), None)
if rootfs is None:
    result["missing"] = 4
    finish("The selected Browser artifact store has no image set." if args.artifact_data_dir else None)
source_vm = rootfs.parent
source_data = source_vm.parent
plan = {rootfs_rel: rootfs, manifest_rel: source_vm / "browser-vm-rootfs-manifest.json"}
for dest, alternatives in [("bin/vmlinux", [source_data / "bin/vmlinux", source_vm / "vmlinux"]),
                           (initrd_rel, [source_data / initrd_rel, source_vm / "initrd"])]:
    plan[dest] = next((p for p in alternatives if p.is_file()), alternatives[0])
# Existing destination files always retain ownership. An incompatible mixture
# fails before setup creates any links; it is repaired by installing a full set.
for rel in plan:
    dest = data / rel
    if dest.is_symlink() and not dest.exists():
        finish(f"Broken Browser artifact link requires repair: {dest}")
    if dest.exists():
        plan[rel] = dest
        result["skipped_existing"] += 1
    if not plan[rel].is_file():
        result["missing"] += 1
if result["missing"]:
    finish("Browser image set is incomplete. Install its rootfs, kernel, initrd and matching build manifest together.")

env = {**os.environ, "ELASTOS_BROWSER_VM_PLATFORM": args.platform,
       "ELASTOS_BROWSER_VM_DATA_DIR": str(data), "ELASTOS_BROWSER_VM_STAGED_ROOTFS": "",
       "ELASTOS_BROWSER_VM_ROOTFS": str(plan[rootfs_rel]),
       "ELASTOS_BROWSER_VM_ROOTFS_MANIFEST": str(plan[manifest_rel]),
       "ELASTOS_BROWSER_VM_KERNEL": str(plan["bin/vmlinux"]),
       ("ELASTOS_BROWSER_VM_INITRAMFS" if args.platform == "darwin-arm64"
        else "ELASTOS_BROWSER_VM_INITRD"): str(plan[initrd_rel])}
try:
    verified = subprocess.run([str(repo_root / "scripts/browser-vm-artifact-preflight.sh"),
                               "--verify-image-set"], env=env, capture_output=True, text=True,
                              timeout=120)
    proof = json.loads(verified.stdout)
    if verified.returncode or proof.get("schema") != "elastos.browser.vm-image-set/v1" or proof.get("ok") is not True:
        finish("Browser image set failed verification. Rebuild or install a verified set; existing artifacts were preserved.")
except (OSError, ValueError, subprocess.SubprocessError) as exc:
    finish(f"Browser image verification could not finish: {exc}")

if args.platform.startswith("linux-"):
    crosvm = source_data / "bin/crosvm"
    if crosvm.is_file():
        plan["bin/crosvm"] = crosvm
    elif not (data / "bin/crosvm").is_file():
        result["missing"] += 1
# Install the receipt last so a first-time consumer cannot admit a partial set.
# This developer projection is read-only; Runtime package admission owns updates.
ordered = [rel for rel in plan if rel != manifest_rel] + [manifest_rel]
created = []
try:
    for rel in ordered:
        dest = data / rel
        if dest.exists():
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.symlink_to(plan[rel].resolve())
        created.append(dest)
except OSError as exc:
    for dest in reversed(created):
        dest.unlink()
    finish(f"Browser artifact links could not be installed: {exc}")
result.update(linked=len(created), image_set_verified=True)
finish()
PY
